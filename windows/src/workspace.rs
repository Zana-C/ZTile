//! Workspaces: groups of windows tiled on a monitor, each with its own taskbar button.
//!
//! One process (the "daemon", living in the tray) owns every workspace. Other
//! invocations find it through a hidden window and forward their command line.
//!
//! Per workspace:
//! - member windows lose their own taskbar buttons (ITaskbarList::DeleteTab)
//! - an always-minimized "representative" window owns one taskbar button with a
//!   composite icon; every click on it arrives as SC_RESTORE and toggles the group
//! - members are never focused (chat apps would send read receipts)
//! - workspaces on the same monitor stack like windows: showing one puts it on
//!   top, minimizing it reveals the one below
//! - a cell remembers its app even after the app closes, so it can be relaunched
//! - opening a saved layout launches the missing apps and catches their windows
//!
//! Dragging (any window, member or not) shows a drop overlay:
//! - onto another cell: swap (or fill if empty), also across workspaces
//! - outside every grid: leave the group
//! - small move / own cell: snap back

use crate::apps::{self, AppSpec};
use crate::layout::{self, NRect, Node};
use crate::store::{SavedLayout, Store};
use crate::{icon, win};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

const WM_ZT_EVENT: u32 = WM_APP + 1;
const WM_ZT_COMMAND: u32 = WM_APP + 2;
pub const WM_ZT_TRAY: u32 = WM_APP + 3;
/// Tag for our WM_COPYDATA payloads.
const COPYDATA_ARGS: usize = 0x5A54_494C; // "ZTIL"
/// Drags shorter than this (px) snap back instead of swapping/detaching.
const SWAP_MIN_MOVE: i32 = 48;
const TIMER_LAUNCH: usize = 1;
/// How long we wait for a launched app's window.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);

const CLASS_DAEMON: PCWSTR = w!("ZTileDaemon");
const CLASS_REP: PCWSTR = w!("ZTileGroup");
const CLASS_PLUS: PCWSTR = w!("ZTilePlus");
const CLASS_OVERLAY: PCWSTR = w!("ZTileOverlay");

// Overlay colours (COLORREF = 0x00BBGGRR)
const COLOR_MOVE: u32 = 0x00D4_7800; // accent blue: swap / fill
const COLOR_STAY: u32 = 0x0080_8080; // grey: snaps back
const COLOR_LEAVE: u32 = 0x0023_11E8; // red: leaves the group

pub struct Setup {
    /// shown in the taskbar tooltip and used to find the saved layout; "" = unnamed
    pub name: String,
    pub tree: Node,
    pub monitor: win::Monitor,
    pub gap: i32,
    pub borderless: bool,
    /// one per cell, in cell order; None = empty cell
    pub hwnds: Vec<Option<HWND>>,
    /// saved app per cell (keeps user choices such as a pinned window); may be empty
    pub specs: Vec<Option<AppSpec>>,
    /// write changes made in the live group (drag, "+") back to the saved layout
    pub keep_changes: bool,
}

struct Slot {
    hwnd: Option<HWND>,
    /// the app this cell belongs to; survives the app closing (for relaunch/saving)
    spec: Option<AppSpec>,
    old_style: Option<u32>,
    /// adopted from a launch: if this window dies before then (a splash screen),
    /// keep waiting for the real one
    armed_until: Option<Instant>,
    plus: HWND,
}

struct Workspace {
    id: u32,
    name: String,
    rep: HWND,
    #[allow(dead_code)] // Phase 8: re-home workspaces when monitors change
    device: String,
    area: RECT,
    ncells: Vec<NRect>,
    cells: Vec<RECT>,
    slots: Vec<Slot>,
    borderless: bool,
    /// the layout's "save changes made in the group" option
    keep_changes: bool,
    minimized: bool,
    icons: Vec<HICON>,
    /// raised last = highest; decides which of several stacked groups a drop hits
    z: u64,
}

impl Workspace {
    fn slot_of(&self, h: HWND) -> Option<usize> {
        self.slots.iter().position(|s| s.hwnd == Some(h))
    }
    fn members(&self) -> Vec<(usize, HWND)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.hwnd.map(|h| (i, h))).collect()
    }
    fn place_slot(&self, i: usize) {
        if let Some(h) = self.slots[i].hwnd {
            win::place(h, self.cells[i]);
        }
    }
    /// Ground truth, not tracked state: is the topmost normal window over our
    /// area one of our members (and none of them minimized)?
    fn on_top(&self) -> bool {
        let members = self.members();
        if self.minimized || members.iter().any(|(_, h)| win::is_minimized(*h)) {
            return false;
        }
        if members.is_empty() {
            return true; // only "+" cells: visible means on top
        }
        for h in win::app_windows() {
            if win::is_minimized(h) || win::is_topmost(h) {
                continue;
            }
            let r = win::frame_rect(h);
            let a = &self.area;
            if r.right <= a.left || r.left >= a.right || r.bottom <= a.top || r.top >= a.bottom {
                continue;
            }
            return self.slot_of(h).is_some();
        }
        true
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drop {
    /// onto an occupied cell
    Swap(u32, usize),
    /// onto an empty cell
    Fill(u32, usize),
    /// back to its own cell
    Stay(u32, usize),
    /// out of its group
    Leave(u32, usize),
}

struct Drag {
    h: HWND,
    from: Option<(u32, usize)>,
    start: RECT,
    target: Option<Drop>,
    /// Some(true) = moving, Some(false) = resizing; decided by the first change.
    /// Later size changes (crossing to a display with another DPI) don't count.
    moving: Option<bool>,
}

/// An app we launched and whose window we're waiting for.
struct Pending {
    ws: u32,
    slot: usize,
    spec: AppSpec,
    /// windows that existed before the launch (never grab those)
    before: Vec<isize>,
    deadline: Instant,
}

struct App {
    spaces: Vec<Workspace>,
    next_id: u32,
    /// counter for Workspace::z
    next_z: u64,
    taskbar: Option<ITaskbarList>,
    /// system-wide hooks for rare events (foreground, drag start/end, minimize)
    hooks: Vec<HWINEVENTHOOK>,
    /// per-process hooks for chatty events (destroy/hide, location), only for the
    /// processes we care about: members and the window being dragged
    obj_hooks: Vec<(u32, [HWINEVENTHOOK; 2])>,
    drag: Option<Drag>,
    pending: Vec<Pending>,
    daemon: HWND,
    overlay: HWND,
    hinst: HINSTANCE,
    msg_taskbar_created: u32,
    on_command: fn(&[String]),
}

impl App {
    fn idx(&self, id: u32) -> Option<usize> {
        self.spaces.iter().position(|w| w.id == id)
    }
    /// Mark workspace `wi` as the one on top of its stack.
    fn raise(&mut self, wi: usize) {
        self.spaces[wi].z = self.next_z;
        self.next_z += 1;
    }
    /// (workspace index, slot) of a member window
    fn locate(&self, h: HWND) -> Option<(usize, usize)> {
        self.spaces.iter().enumerate().find_map(|(wi, w)| w.slot_of(h).map(|s| (wi, s)))
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// member hwnds for the hook's cheap pre-filter
    static MEMBERS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// window being dragged: its LOCATIONCHANGE events pass the pre-filter
    static DRAG_HWND: Cell<isize> = const { Cell::new(0) };
    /// true while *we* resize/minimize a representative window
    static SELF_SIZE: Cell<bool> = const { Cell::new(false) };
    static LAST_TOGGLE: Cell<Option<(u32, Instant)>> = const { Cell::new(None) };
    static OVERLAY_COLOR: Cell<u32> = const { Cell::new(COLOR_MOVE) };
    static COMMANDS: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
}
static DAEMON: AtomicIsize = AtomicIsize::new(0);
static DONE: AtomicBool = AtomicBool::new(false);

/// Run `f` on the app state. None if not initialised or busy (re-entrant call).
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut a) => a.as_mut().map(f),
        Err(_) => None,
    })
}

fn taskbar() -> Option<ITaskbarList> {
    unsafe {
        let t: ITaskbarList = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
        t.HrInit().ok()?;
        Some(t)
    }
}

// ---------------------------------------------------------------- daemon / IPC

/// The running daemon's hidden window, if any. Must belong to a ztile.exe process:
/// another program creating a window with our class name can't stop us starting
/// or receive our commands.
pub fn daemon_window() -> Option<HWND> {
    let h = unsafe { FindWindowW(CLASS_DAEMON, None).ok().filter(|h| !h.is_invalid())? };
    let ours = std::env::current_exe().ok()?.file_name()?.to_string_lossy().to_lowercase();
    win::exe_name(h).eq_ignore_ascii_case(&ours).then_some(h)
}

pub fn daemon_hwnd() -> HWND {
    HWND(DAEMON.load(Ordering::Relaxed) as *mut _)
}

/// Send our command line to the running daemon.
pub fn forward(daemon: HWND, args: &[String]) -> bool {
    let payload: Vec<u16> = args.join("\0").encode_utf16().collect();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_ARGS,
        cbData: (payload.len() * 2) as u32,
        lpData: payload.as_ptr() as *mut _,
    };
    let mut res = 0usize;
    unsafe {
        SendMessageTimeoutW(
            daemon,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut res),
        )
        .0 != 0
            && res == 1
    }
}

/// Become the daemon: window classes, hidden window, drop overlay, event hooks.
pub fn init(on_command: fn(&[String])) -> bool {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        type Proc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;
        for (class, proc_, cursor) in [
            (CLASS_DAEMON, daemon_proc as Proc, arrow),
            (CLASS_REP, rep_proc as Proc, arrow),
            (CLASS_PLUS, plus_proc as Proc, LoadCursorW(None, IDC_HAND).unwrap_or_default()),
            (CLASS_OVERLAY, overlay_proc as Proc, arrow),
        ] {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: hinst,
                lpszClassName: class,
                hCursor: cursor,
                ..Default::default()
            });
        }
        // A real (never shown) top-level window, not message-only: it must get the
        // TaskbarCreated broadcast to restore the tray icon after Explorer restarts.
        let Ok(daemon) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS_DAEMON,
            w!("ZTile"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            return false;
        };
        DAEMON.store(daemon.0 as isize, Ordering::SeqCst);

        let overlay = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            CLASS_OVERLAY,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        )
        .unwrap_or_default();
        let _ = SetLayeredWindowAttributes(overlay, COLORREF(0), 90, LWA_ALPHA);

        // Only rare system events are watched system-wide. Object events (every
        // mouse move produces some) are hooked per process in sync_hooks().
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let hooks = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZESTART),
        ]
        .iter()
        .map(|&(a, b)| SetWinEventHook(a, b, None, Some(hook), 0, 0, flags))
        .collect();

        let app = App {
            spaces: Vec::new(),
            next_id: 1,
            next_z: 1,
            taskbar: taskbar(),
            hooks,
            obj_hooks: Vec::new(),
            drag: None,
            pending: Vec::new(),
            daemon,
            overlay,
            hinst,
            msg_taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            on_command,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    // We build with panic=abort: if anything ever panics, give the windows back
    // (taskbar buttons, corners, minimized state) before the process dies.
    std::panic::set_hook(Box::new(|_| emergency_restore()));
    true
}

/// Last-resort cleanup that needs nothing but the member list.
fn emergency_restore() {
    let members: Vec<isize> = MEMBERS.with(|m| m.try_borrow().map(|m| m.clone()).unwrap_or_default());
    let tb = taskbar();
    for hv in members {
        let h = HWND(hv as *mut _);
        if !win::is_alive(h) {
            continue;
        }
        unsafe {
            if let Some(tb) = &tb {
                let _ = tb.AddTab(h);
            }
            if win::is_minimized(h) {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
        }
        win::set_square_corners(h, false);
    }
    crate::tray::remove();
}

pub fn run_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    shutdown();
    DONE.store(true, Ordering::SeqCst);
}

/// Close every workspace and leave the message loop.
pub fn quit() {
    unsafe { PostQuitMessage(0) };
}

/// Names of the open (named) workspaces.
pub fn open_names() -> Vec<String> {
    with(|a| a.spaces.iter().filter(|w| !w.name.is_empty()).map(|w| w.name.clone()).collect()).unwrap_or_default()
}

pub fn is_open(name: &str) -> bool {
    open_names().iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn id_by_name(name: &str) -> Option<u32> {
    with(|a| a.spaces.iter().find(|w| w.name.eq_ignore_ascii_case(name)).map(|w| w.id)).flatten()
}

/// Bring a workspace with this name up. false if none is open.
pub fn show_named(name: &str) -> bool {
    let Some(id) = id_by_name(name) else { return false };
    restore_ws(id);
    true
}

pub fn close_named(name: &str) {
    if let Some(id) = id_by_name(name) {
        close_ws(id);
    }
}

// ---------------------------------------------------------------- workspaces

/// Open a saved layout: reuse running apps, launch the missing ones.
/// Already open: just bring it up.
pub fn open_saved(l: &SavedLayout, monitor: Option<win::Monitor>) {
    if show_named(&l.name) {
        return;
    }
    let mons = win::monitors();
    let Some(mon) = monitor.or_else(|| l.monitor.find(&mons)) else { return };
    let hwnds = apps::match_windows(&l.apps);
    let Some(id) = open(Setup {
        name: l.name.clone(),
        tree: l.tree.clone(),
        monitor: mon,
        gap: l.gap,
        borderless: l.borderless,
        hwnds,
        specs: l.apps.clone(),
        keep_changes: l.keep_changes,
    }) else {
        return;
    };
    // Empty cells still belong to their saved apps: remember them and launch them.
    let missing: Vec<(usize, AppSpec)> = with(|a| {
        let Some(wi) = a.idx(id) else { return Vec::new() };
        let ws = &mut a.spaces[wi];
        let mut out = Vec::new();
        for (i, spec) in l.apps.iter().enumerate().take(ws.slots.len()) {
            if ws.slots[i].hwnd.is_none() {
                ws.slots[i].spec = spec.clone();
                if let Some(s) = spec {
                    out.push((i, s.clone()));
                }
            }
        }
        out
    })
    .unwrap_or_default();
    refresh_saved_names(id);
    for (slot, spec) in missing {
        launch_into(id, slot, spec);
    }
}

/// Create a workspace and tile its windows. Returns its id.
pub fn open(setup: Setup) -> Option<u32> {
    let Setup { name, tree, monitor: mon, gap, borderless, mut hwnds, mut specs, keep_changes } = setup;
    let ncells = tree.cells();
    hwnds.resize(ncells.len(), None);
    specs.resize(ncells.len(), None);
    let cells = layout::to_pixels(&ncells, mon.work, gap);

    // A window can only live in one workspace: take it out of its old one.
    let mut touched = Vec::new();
    with(|a| {
        for h in hwnds.iter().flatten() {
            if let Some((wi, s)) = a.locate(*h) {
                let tb = a.taskbar.clone();
                release(&tb, &mut a.spaces[wi], s, false, false);
                touched.push(a.spaces[wi].id);
            }
        }
    });
    for id in touched {
        refresh_icon(id);
        persist(id);
    }

    let id = with(|a| {
        let id = a.next_id;
        a.next_id += 1;
        unsafe {
            let rep = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_APPWINDOW,
                CLASS_REP,
                w!("ZTile"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                mon.work.left,
                mon.work.top,
                1,
                1,
                None,
                None,
                Some(a.hinst),
                None,
            )
            .unwrap_or_default();
            let _ = SetLayeredWindowAttributes(rep, COLORREF(0), 0, LWA_ALPHA);
            SetWindowLongPtrW(rep, GWLP_USERDATA, id as isize);
            // Own AppUserModelID per group, set before the window is shown: otherwise
            // the taskbar combines all our groups into one button.
            apps::set_aumid(rep, &apps::workspace_aumid(&name, id));

            let mut slots = Vec::new();
            for (i, c) in cells.iter().enumerate() {
                let plus = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    CLASS_PLUS,
                    w!(""),
                    WS_POPUP,
                    c.left,
                    c.top,
                    c.right - c.left,
                    c.bottom - c.top,
                    None,
                    None,
                    Some(a.hinst),
                    None,
                )
                .unwrap_or_default();
                SetWindowLongPtrW(plus, GWLP_USERDATA, ((id as isize) << 16) | i as isize);
                slots.push(Slot { hwnd: None, spec: specs[i].clone(), old_style: None, armed_until: None, plus });
            }
            a.spaces.push(Workspace {
                id,
                name: name.clone(),
                rep,
                device: mon.device.clone(),
                area: mon.work,
                ncells,
                cells,
                slots,
                borderless,
                keep_changes,
                minimized: false,
                icons: Vec::new(),
                z: 0,
            });
            keep_rep_minimized(rep);
        }
        let tb = a.taskbar.clone();
        let ws = a.spaces.last_mut().unwrap();
        for (i, h) in hwnds.iter().enumerate() {
            match h {
                Some(h) => adopt(&tb, ws, i, *h),
                None => unsafe {
                    let _ = ShowWindow(ws.slots[i].plus, SW_SHOWNOACTIVATE);
                },
            }
        }
        // Bring them up but never focus them: a focused chat app marks messages as read.
        for (_, h) in ws.members() {
            win::raise_no_activate(h);
        }
        for s in &ws.slots {
            if s.hwnd.is_none() {
                win::raise_no_activate(s.plus);
            }
        }
        let wi = a.spaces.len() - 1;
        a.raise(wi);
        sync_members(a);
        id
    })?;
    refresh_icon(id);
    crate::ui::refresh();
    Some(id)
}

/// Take ownership of a window for slot `i`: hide its taskbar button, place it.
fn adopt(tb: &Option<ITaskbarList>, ws: &mut Workspace, i: usize, h: HWND) {
    ws.slots[i].hwnd = Some(h);
    let mut spec = AppSpec::of(h);
    // same app as the cell already had: keep the user's choices (pinned window)
    if let Some(old) = &ws.slots[i].spec {
        if old.same_app(&spec) {
            spec.window = old.window.clone();
        }
    }
    ws.slots[i].spec = Some(spec);
    if let Some(tb) = tb {
        unsafe {
            let _ = tb.DeleteTab(h);
        }
    }
    // Borderless only works for classic system title bars. Apps that draw their
    // own (Chromium/Electron) keep theirs: their buttons live up there.
    if ws.borderless && ws.slots[i].old_style.is_none() && win::native_caption_height(h) > 4 {
        win::set_border_hidden(h, true);
        ws.slots[i].old_style = Some(win::make_borderless(h));
    }
    if ws.minimized {
        unsafe {
            let _ = ShowWindow(h, SW_SHOWMINNOACTIVE);
        }
    } else {
        ws.place_slot(i);
    }
    win::set_square_corners(h, true);
    unsafe {
        let _ = ShowWindow(ws.slots[i].plus, SW_HIDE);
    }
}

/// Release slot `i`.
/// - `give_back_tab`: the window stays a normal window, so it gets its taskbar button back
/// - `keep_spec`: the cell still belongs to that app (it just closed); otherwise forget it
fn release(tb: &Option<ITaskbarList>, ws: &mut Workspace, i: usize, give_back_tab: bool, keep_spec: bool) {
    if !keep_spec {
        ws.slots[i].spec = None;
    }
    ws.slots[i].armed_until = None;
    let Some(h) = ws.slots[i].hwnd.take() else { return };
    let old_style = ws.slots[i].old_style.take();
    if win::is_alive(h) {
        if let Some(st) = old_style {
            win::restore_style(h, st);
            win::set_border_hidden(h, false);
        }
        win::set_square_corners(h, false);
        if give_back_tab {
            if let Some(tb) = tb {
                unsafe {
                    let _ = tb.AddTab(h);
                }
            }
        }
    }
    if !ws.minimized {
        let p = ws.slots[i].plus;
        unsafe {
            let _ = ShowWindow(p, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(p), None, true);
        }
        win::raise_no_activate(p);
    }
}

fn sync_members(a: &mut App) {
    let m: Vec<isize> = a.spaces.iter().flat_map(|w| w.members()).map(|(_, h)| h.0 as isize).collect();
    MEMBERS.with(|x| *x.borrow_mut() = m);
    sync_hooks(a);
}

/// Keep exactly one pair of object-event hooks per process we watch.
fn sync_hooks(a: &mut App) {
    let mut want: Vec<u32> = a.spaces.iter().flat_map(|w| w.members()).map(|(_, h)| win::pid(h)).collect();
    if let Some(d) = &a.drag {
        want.push(win::pid(d.h));
    }
    want.retain(|p| *p != 0);
    want.sort_unstable();
    want.dedup();
    a.obj_hooks.retain(|(pid, hs)| {
        if want.contains(pid) {
            return true;
        }
        for h in hs {
            unsafe {
                let _ = UnhookWinEvent(*h);
            }
        }
        false
    });
    for pid in want {
        if a.obj_hooks.iter().any(|(p, _)| *p == pid) {
            continue;
        }
        let f = WINEVENT_OUTOFCONTEXT;
        let hs = unsafe {
            [
                SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, None, Some(hook), pid, 0, f),
                SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, None, Some(hook), pid, 0, f),
            ]
        };
        a.obj_hooks.push((pid, hs));
    }
}

/// Write a named workspace's current apps back to its saved layout, if that
/// layout asks for it ("save changes made in the group"); otherwise the saved
/// layout stays exactly as it was edited. Called after deliberate changes
/// (drag, "+"), not when an app merely closes.
fn persist(id: u32) {
    let Some((name, specs)) = with(|a| {
        let ws = &a.spaces[a.idx(id)?];
        if ws.name.is_empty() || !ws.keep_changes {
            return None;
        }
        Some((ws.name.clone(), ws.slots.iter().map(|s| s.spec.clone()).collect::<Vec<_>>()))
    })
    .flatten() else {
        return;
    };
    let mut st = Store::load();
    let Some(l) = st.layouts.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&name)) else { return };
    if l.apps == specs {
        return;
    }
    l.apps = specs;
    let _ = st.save();
    crate::ui::refresh();
}

/// Housekeeping only, never changes which apps a layout has: fill in app names
/// for cells whose app is running (older files had none, and kept window titles,
/// which this save drops).
fn refresh_saved_names(id: u32) {
    let Some((name, live)) = with(|a| {
        let ws = &a.spaces[a.idx(id)?];
        let live: Vec<Option<AppSpec>> =
            ws.slots.iter().map(|s| s.hwnd.and_then(|_| s.spec.clone())).collect();
        (!ws.name.is_empty()).then(|| (ws.name.clone(), live))
    })
    .flatten() else {
        return;
    };
    let mut st = Store::load();
    let Some(l) = st.layouts.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&name)) else { return };
    let mut changed = false;
    for (saved, live) in l.apps.iter_mut().zip(&live) {
        if let (Some(s), Some(lv)) = (saved.as_mut(), live) {
            if s.same_app(lv) && !lv.name.is_empty() && s.name != lv.name {
                s.name = lv.name.clone();
                changed = true;
            }
        }
    }
    if changed {
        let _ = st.save();
        crate::ui::refresh();
    }
}

fn refresh_icon(id: u32) {
    let Some((rep, name, ncells, hw, old)) = with(|a| {
        let i = a.idx(id)?;
        let ws = &mut a.spaces[i];
        let hw: Vec<Option<HWND>> = ws.slots.iter().map(|s| s.hwnd).collect();
        Some((ws.rep, ws.name.clone(), ws.ncells.clone(), hw, std::mem::take(&mut ws.icons)))
    })
    .flatten() else {
        return;
    };

    // Icon lookups talk to other processes: do them without holding the state.
    let icons: Vec<Option<HICON>> = hw.iter().map(|h| h.map(icon::window_icon)).collect();
    unsafe {
        let dpi = GetDpiForWindow(rep);
        let big = icon::composite(&ncells, &icons, GetSystemMetricsForDpi(SM_CXICON, dpi));
        let small = icon::composite(&ncells, &icons, GetSystemMetricsForDpi(SM_CXSMICON, dpi));
        SendMessageW(rep, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(big.0 as isize)));
        SendMessageW(rep, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(small.0 as isize)));
        for ic in old {
            let _ = DestroyIcon(ic);
        }
        let names: Vec<String> = hw
            .iter()
            .map(|h| match h {
                Some(h) => win::exe_name(*h).trim_end_matches(".exe").to_string(),
                None => "+".into(),
            })
            .collect();
        let head = if name.is_empty() { "ZTile".to_string() } else { name };
        let t = win::wide(&format!("{head} · {}", names.join(" | ")));
        let _ = SetWindowTextW(rep, PCWSTR(t.as_ptr()));
        // keep them so the next refresh / close can free them
        with(|a| {
            if let Some(i) = a.idx(id) {
                a.spaces[i].icons = vec![big, small];
            }
        });
    }
}

/// The representative window is *always* minimized:
/// - Windows never hands it focus on its own (e.g. when another window minimizes),
/// - so every click on its taskbar button arrives as a "restore" request,
///   which we turn into a toggle without actually restoring it.
fn keep_rep_minimized(rep: HWND) {
    if !win::is_minimized(rep) {
        SELF_SIZE.set(true);
        unsafe {
            let _ = ShowWindow(rep, SW_SHOWMINNOACTIVE);
        }
        SELF_SIZE.set(false);
    }
}

/// Taskbar button clicked: group on top -> minimize it, otherwise bring it up.
fn toggle(id: u32) {
    let now = Instant::now();
    // SC_RESTORE and the resulting WM_SIZE can both arrive for one click.
    if LAST_TOGGLE.get().is_some_and(|(i, t)| i == id && now.duration_since(t).as_millis() < 300) {
        return;
    }
    LAST_TOGGLE.set(Some((id, now)));
    let on_top = with(|a| a.idx(id).map(|i| a.spaces[i].on_top())).flatten().unwrap_or(false);
    if on_top { minimize_ws(id) } else { restore_ws(id) }
}

fn minimize_ws(id: u32) {
    let Some((rep, members, plus)) = with(|a| {
        let i = a.idx(id)?;
        let ws = &mut a.spaces[i];
        ws.minimized = true;
        let plus: Vec<HWND> = ws.slots.iter().map(|s| s.plus).collect();
        Some((ws.rep, ws.members(), plus))
    })
    .flatten() else {
        return;
    };
    unsafe {
        for (_, h) in members {
            let _ = ShowWindow(h, SW_SHOWMINNOACTIVE);
        }
        for p in plus {
            let _ = ShowWindow(p, SW_HIDE);
        }
    }
    keep_rep_minimized(rep);
}

/// Show a workspace on top without activating any member. Other workspaces on
/// the same monitor just end up behind it (minimizing this one reveals them).
fn restore_ws(id: u32) {
    let Some((members, plus)) = with(|a| {
        let i = a.idx(id)?;
        a.raise(i);
        let ws = &mut a.spaces[i];
        ws.minimized = false;
        for (i, h) in ws.members() {
            if win::is_minimized(h) {
                unsafe {
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
            }
            ws.place_slot(i);
        }
        let plus: Vec<HWND> = ws.slots.iter().filter(|s| s.hwnd.is_none()).map(|s| s.plus).collect();
        Some((ws.members(), plus))
    })
    .flatten() else {
        return;
    };
    for (_, h) in &members {
        win::raise_no_activate(*h);
    }
    for p in plus {
        unsafe {
            let _ = ShowWindow(p, SW_SHOWNOACTIVATE);
        }
        win::raise_no_activate(p);
    }
}

/// Dissolve a workspace: windows get their buttons and corners back.
fn close_ws(id: u32) {
    let Some(ws) = with(|a| {
        let i = a.idx(id)?;
        let mut ws = a.spaces.remove(i);
        let tb = a.taskbar.clone();
        ws.minimized = true; // don't flash "+" windows while releasing
        for i in 0..ws.slots.len() {
            if let Some(h) = ws.slots[i].hwnd {
                if win::is_minimized(h) {
                    unsafe {
                        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                    }
                }
            }
            release(&tb, &mut ws, i, true, false);
        }
        a.pending.retain(|p| p.ws != id);
        sync_members(a);
        Some(ws)
    })
    .flatten() else {
        return;
    };
    unsafe {
        for s in &ws.slots {
            let _ = DestroyWindow(s.plus);
        }
        for ic in &ws.icons {
            if !ic.is_invalid() {
                let _ = DestroyIcon(*ic);
            }
        }
        apps::set_aumid(ws.rep, ""); // the shell asks for this before destroy
        let _ = DestroyWindow(ws.rep);
    }
    crate::ui::refresh();
}

fn shutdown() {
    let ids: Vec<u32> = with(|a| a.spaces.iter().map(|w| w.id).collect()).unwrap_or_default();
    for id in ids {
        close_ws(id);
    }
    crate::tray::remove();
    crate::ui::close();
    with(|a| unsafe {
        for h in a.hooks.drain(..) {
            let _ = UnhookWinEvent(h);
        }
        for (_, hs) in a.obj_hooks.drain(..) {
            for h in hs {
                let _ = UnhookWinEvent(h);
            }
        }
        let _ = DestroyWindow(a.overlay);
    });
}

// ---------------------------------------------------------------- launching

/// Start an app for a cell and wait (briefly) for its window.
fn launch_into(id: u32, slot: usize, spec: AppSpec) {
    let before: Vec<isize> = win::app_windows().iter().map(|h| h.0 as isize).collect();
    // ShellExecute may pump messages: never call it while holding the state.
    if !apps::launch(&spec) {
        eprintln!("  ! couldn't start: {}", spec.label());
        return;
    }
    arm(id, slot, spec, before);
}

fn arm(id: u32, slot: usize, spec: AppSpec, before: Vec<isize>) {
    with(|a| {
        a.pending.retain(|p| !(p.ws == id && p.slot == slot));
        a.pending.push(Pending { ws: id, slot, spec, before, deadline: Instant::now() + LAUNCH_WAIT });
        unsafe {
            SetTimer(Some(a.daemon), TIMER_LAUNCH, 250, None);
        }
    });
}

/// Timer tick while launches are pending: catch new windows that match.
fn launch_tick() {
    let now = Instant::now();
    let mut touched = Vec::new();
    with(|a| {
        let live: Vec<u32> = a.spaces.iter().map(|w| w.id).collect();
        a.pending.retain(|p| p.deadline > now && live.contains(&p.ws));
        if a.pending.is_empty() {
            unsafe {
                let _ = KillTimer(Some(a.daemon), TIMER_LAUNCH);
            }
            return;
        }
        let members: Vec<isize> = MEMBERS.with(|m| m.borrow().clone());
        // Only windows that are new for at least one launch: the expensive part
        // (process + property queries) runs for a handful of windows, not all.
        let fresh: Vec<(HWND, AppSpec, String)> = win::app_windows()
            .into_iter()
            .filter(|h| {
                let hv = h.0 as isize;
                !members.contains(&hv) && a.pending.iter().any(|p| !p.before.contains(&hv)) && !win::is_fullscreen(*h)
            })
            .map(|h| (h, AppSpec::of(h), win::title(h)))
            .collect();
        let tb = a.taskbar.clone();
        let mut done = Vec::new();
        let mut used: Vec<HWND> = Vec::new();
        for (pi, p) in a.pending.iter().enumerate() {
            let best = fresh
                .iter()
                .filter(|(h, _, _)| !p.before.contains(&(h.0 as isize)) && !used.contains(h))
                .filter_map(|(h, w, t)| p.spec.score(w, t).map(|s| (s, *h)))
                .max_by_key(|(s, _)| *s);
            let Some((_, h)) = best else { continue };
            let Some(wi) = a.idx(p.ws) else { continue };
            let ws = &mut a.spaces[wi];
            if p.slot < ws.slots.len() && ws.slots[p.slot].hwnd.is_none() {
                adopt(&tb, ws, p.slot, h);
                ws.slots[p.slot].armed_until = Some(p.deadline);
                win::raise_no_activate(h);
                touched.push(p.ws);
            }
            used.push(h);
            done.push(pi);
        }
        for pi in done.into_iter().rev() {
            a.pending.remove(pi);
        }
        sync_members(a);
    });
    touched.dedup();
    for id in touched {
        refresh_icon(id);
    }
}

// ---------------------------------------------------------------- dragging

fn drop_target(a: &App, d: &Drag, pt: POINT) -> Option<Drop> {
    let (s, f) = (d.start, win::frame_rect(d.h));
    let same_size = s.right - s.left == f.right - f.left && s.bottom - s.top == f.bottom - f.top;
    if !d.moving.unwrap_or(same_size) {
        return None; // resizing, not moving (Phase 7 will resize neighbours)
    }
    let moved = (f.left - s.left).abs() + (f.top - s.top).abs();
    // Groups on the same display overlap: the one on top is the one you see.
    let hit = a
        .spaces
        .iter()
        .filter(|w| !w.minimized)
        .filter_map(|w| w.cells.iter().position(|c| win::contains(c, pt)).map(|j| (w, j)))
        .max_by_key(|(w, _)| w.z);
    match (d.from, hit) {
        (Some((fid, i)), Some((w, j))) => {
            if (w.id == fid && j == i) || moved < SWAP_MIN_MOVE {
                Some(Drop::Stay(fid, i))
            } else if w.slots[j].hwnd.is_none() {
                Some(Drop::Fill(w.id, j))
            } else {
                Some(Drop::Swap(w.id, j))
            }
        }
        (Some((fid, i)), None) => {
            let own = a.idx(fid).map(|x| win::contains(&a.spaces[x].area, pt)).unwrap_or(false);
            if own || moved < SWAP_MIN_MOVE { Some(Drop::Stay(fid, i)) } else { Some(Drop::Leave(fid, i)) }
        }
        (None, Some((w, j))) if w.slots[j].hwnd.is_none() => Some(Drop::Fill(w.id, j)),
        _ => None,
    }
}

fn cursor() -> POINT {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    pt
}

fn drag_start(h: HWND) {
    let is_member = with(|a| a.locate(h).is_some()).unwrap_or(false);
    if !is_member && !win::is_app_window(h) {
        return;
    }
    DRAG_HWND.set(h.0 as isize);
    with(|a| {
        let from = a.locate(h).map(|(wi, s)| (a.spaces[wi].id, s));
        a.drag = Some(Drag { h, from, start: win::frame_rect(h), target: None, moving: None });
        sync_hooks(a); // start hearing this window's moves for the overlay
    });
}

fn drag_move(h: HWND) {
    let pt = cursor();
    let change = with(|a| {
        let d = a.drag.as_mut().filter(|d| d.h == h)?;
        if d.moving.is_none() {
            let (s, f) = (d.start, win::frame_rect(h));
            if s != f {
                d.moving = Some(s.right - s.left == f.right - f.left && s.bottom - s.top == f.bottom - f.top);
            }
        }
        let d = a.drag.as_ref()?;
        let t = drop_target(a, d, pt);
        if t == d.target {
            return None;
        }
        let rect = t.and_then(|t| {
            let (Drop::Swap(id, j) | Drop::Fill(id, j) | Drop::Stay(id, j) | Drop::Leave(id, j)) = t;
            a.idx(id).map(|i| a.spaces[i].cells[j])
        });
        a.drag.as_mut().unwrap().target = t;
        Some((a.overlay, t, rect))
    })
    .flatten();
    if let Some((ov, t, rect)) = change {
        show_overlay(ov, t, rect);
    }
}

fn show_overlay(ov: HWND, t: Option<Drop>, rect: Option<RECT>) {
    unsafe {
        let (Some(t), Some(r)) = (t, rect) else {
            let _ = ShowWindow(ov, SW_HIDE);
            return;
        };
        OVERLAY_COLOR.set(match t {
            Drop::Swap(..) | Drop::Fill(..) => COLOR_MOVE,
            Drop::Stay(..) => COLOR_STAY,
            Drop::Leave(..) => COLOR_LEAVE,
        });
        let _ = SetWindowPos(
            ov,
            Some(HWND_TOPMOST),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(Some(ov), None, true);
    }
}

fn drag_end(h: HWND) {
    let pt = cursor();
    let Some((d, ov, target)) = with(|a| {
        if a.drag.as_ref().is_none_or(|d| d.h != h) {
            return None;
        }
        let d = a.drag.take()?;
        let t = drop_target(a, &d, pt);
        Some((d, a.overlay, t))
    })
    .flatten() else {
        return;
    };
    DRAG_HWND.set(0);
    show_overlay(ov, None, None);

    let mut touched: Vec<u32> = Vec::new();
    with(|a| {
        let tb = a.taskbar.clone();
        let from = d.from.and_then(|(id, i)| a.idx(id).map(|wi| (wi, i)));
        match (from, target) {
            // resize or unknown: put a member back where it belongs
            (Some((wi, i)), None | Some(Drop::Stay(..))) => a.spaces[wi].place_slot(i),
            (Some((wi, i)), Some(Drop::Leave(..))) => {
                release(&tb, &mut a.spaces[wi], i, true, false);
                touched.push(a.spaces[wi].id);
            }
            (Some((wi, i)), Some(Drop::Swap(tid, j) | Drop::Fill(tid, j))) => {
                let Some(wj) = a.idx(tid) else { return };
                if wi == wj {
                    // apps (with everything they own) move, "+" windows stay with their cells
                    let ws = &mut a.spaces[wi];
                    let empty_target = ws.slots[j].hwnd.is_none();
                    ws.slots.swap(i, j);
                    let (pi, pj) = (ws.slots[i].plus, ws.slots[j].plus);
                    ws.slots[i].plus = pj;
                    ws.slots[j].plus = pi;
                    ws.place_slot(j);
                    if empty_target {
                        unsafe {
                            let _ = ShowWindow(ws.slots[i].plus, SW_SHOWNOACTIVATE);
                            let _ = ShowWindow(ws.slots[j].plus, SW_HIDE);
                            let _ = InvalidateRect(Some(ws.slots[i].plus), None, true);
                        }
                        win::raise_no_activate(ws.slots[i].plus);
                    } else {
                        ws.place_slot(i);
                    }
                } else {
                    // across workspaces: the two windows trade places
                    let other = a.spaces[wj].slots[j].hwnd;
                    release(&tb, &mut a.spaces[wi], i, false, false);
                    release(&tb, &mut a.spaces[wj], j, false, false);
                    adopt(&tb, &mut a.spaces[wj], j, h);
                    if let Some(o) = other {
                        adopt(&tb, &mut a.spaces[wi], i, o);
                    }
                }
                touched.push(a.spaces[wi].id);
                touched.push(tid);
            }
            (None, Some(Drop::Fill(tid, j))) => {
                let Some(wj) = a.idx(tid) else { return };
                adopt(&tb, &mut a.spaces[wj], j, h);
                touched.push(tid);
            }
            _ => {}
        }
        sync_members(a);
    });
    touched.dedup();
    for id in touched {
        refresh_icon(id);
        persist(id);
    }
}

// ---------------------------------------------------------------- events

fn on_event(ev: u32, h: HWND) {
    match ev {
        EVENT_SYSTEM_FOREGROUND => {
            with(|a| {
                if let Some((wi, _)) = a.locate(h) {
                    a.spaces[wi].minimized = false;
                    a.raise(wi);
                }
            });
        }
        EVENT_SYSTEM_MOVESIZESTART => drag_start(h),
        EVENT_SYSTEM_MOVESIZEEND => drag_end(h),
        EVENT_SYSTEM_MINIMIZESTART => {
            // Minimizing one member minimizes its whole group (its own button is gone).
            let id = with(|a| a.locate(h).filter(|(wi, _)| !a.spaces[*wi].minimized).map(|(wi, _)| a.spaces[wi].id))
                .flatten();
            if let Some(id) = id {
                if win::is_minimized(h) {
                    minimize_ws(id);
                }
            }
        }
        EVENT_OBJECT_DESTROY | EVENT_OBJECT_HIDE => {
            if DRAG_HWND.get() == h.0 as isize {
                DRAG_HWND.set(0);
                let ov = with(|a| {
                    a.drag = None;
                    sync_hooks(a);
                    a.overlay
                });
                if let Some(ov) = ov {
                    show_overlay(ov, None, None);
                }
            }
            // The app closed (or went to the tray): the cell keeps remembering it.
            let res = with(|a| {
                let (wi, s) = a.locate(h)?;
                let tb = a.taskbar.clone();
                let ws = &mut a.spaces[wi];
                let rearm = ws.slots[s].armed_until.filter(|t| *t > Instant::now()).zip(ws.slots[s].spec.clone());
                release(&tb, ws, s, ev == EVENT_OBJECT_HIDE, true);
                sync_members(a);
                Some((a.spaces[wi].id, s, rearm))
            })
            .flatten();
            if let Some((id, s, rearm)) = res {
                if let Some((_, spec)) = rearm {
                    // probably a splash screen: keep waiting for the real window
                    let before = win::app_windows().iter().map(|h| h.0 as isize).collect();
                    arm(id, s, spec, before);
                }
                refresh_icon(id);
            }
        }
        EVENT_OBJECT_LOCATIONCHANGE => {
            if DRAG_HWND.get() == h.0 as isize {
                drag_move(h);
                return;
            }
            // Fight maximize (double-click on title etc.); normal moves end in MOVESIZEEND.
            with(|a| {
                if let Some((wi, s)) = a.locate(h) {
                    if win::is_maximized(h) {
                        a.spaces[wi].place_slot(s);
                    }
                }
            });
        }
        _ => {}
    }
}

unsafe extern "system" fn hook(_: HWINEVENTHOOK, ev: u32, h: HWND, obj: i32, child: i32, _: u32, _: u32) {
    if obj != 0 || child != 0 || h.is_invalid() || ev == EVENT_OBJECT_SHOW {
        return; // only whole windows (OBJID_WINDOW, CHILDID_SELF); SHOW is in the hooked range but unused
    }
    if ev == EVENT_OBJECT_LOCATIONCHANGE || ev == EVENT_OBJECT_HIDE || ev == EVENT_OBJECT_DESTROY {
        // very chatty: drop everything but members and the dragged window right here
        let hv = h.0 as isize;
        let keep = DRAG_HWND.get() == hv
            || MEMBERS.with(|m| m.try_borrow().map(|m| m.contains(&hv)).unwrap_or(true));
        if !keep {
            return;
        }
    }
    unsafe {
        let _ = PostMessageW(Some(daemon_hwnd()), WM_ZT_EVENT, WPARAM(ev as usize), LPARAM(h.0 as isize));
    }
}

unsafe extern "system" fn on_ctrl(_: u32) -> BOOL {
    unsafe {
        let _ = PostMessageW(Some(daemon_hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    // Console close kills us when this returns: give cleanup a moment.
    for _ in 0..50 {
        if DONE.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    true.into()
}

// ---------------------------------------------------------------- window procs

unsafe extern "system" fn daemon_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ZT_EVENT => on_event(wp.0 as u32, HWND(lp.0 as *mut _)),
            WM_ZT_TRAY => crate::tray::on_message(lp),
            WM_TIMER if wp.0 == TIMER_LAUNCH => launch_tick(),
            WM_COPYDATA => {
                if lp.0 == 0 {
                    return LRESULT(0);
                }
                let cds = &*(lp.0 as *const COPYDATASTRUCT);
                // Only our own small, well-formed payloads; everything else is ignored.
                let len = cds.cbData as usize;
                if cds.dwData != COPYDATA_ARGS || cds.lpData.is_null() || len == 0 || len % 2 != 0 || len > 8192 {
                    return LRESULT(0);
                }
                // read as bytes: the sender's buffer isn't guaranteed to be u16-aligned
                let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, len);
                let data: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let args: Vec<String> = String::from_utf16_lossy(&data).split('\0').take(64).map(str::to_string).collect();
                if !matches!(args.first().map(String::as_str), Some("grid" | "open" | "show" | "quit")) {
                    return LRESULT(0);
                }
                // Run it from the message loop, not inside the sender's SendMessage.
                COMMANDS.with(|p| p.borrow_mut().push(args));
                let _ = PostMessageW(Some(h), WM_ZT_COMMAND, WPARAM(0), LPARAM(0));
                // let the sender's window (if any) come to the front later
                return LRESULT(1);
            }
            WM_ZT_COMMAND => {
                let jobs: Vec<Vec<String>> = COMMANDS.with(|p| std::mem::take(&mut *p.borrow_mut()));
                let cmd = with(|a| a.on_command);
                if let Some(cmd) = cmd {
                    for args in jobs {
                        cmd(&args);
                    }
                }
            }
            WM_CLOSE => quit(),
            _ => {
                let tb_created = with(|a| a.msg_taskbar_created).unwrap_or(0);
                if msg != 0 && msg == tb_created {
                    // Explorer restarted: tray icon and our DeleteTab calls are gone.
                    crate::tray::add();
                    with(|a| {
                        a.taskbar = taskbar();
                        if let Some(tb) = &a.taskbar {
                            for ws in &a.spaces {
                                for (_, m) in ws.members() {
                                    let _ = tb.DeleteTab(m);
                                }
                            }
                        }
                    });
                }
                return DefWindowProcW(h, msg, wp, lp);
            }
        }
        LRESULT(0)
    }
}

unsafe extern "system" fn rep_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let id = GetWindowLongPtrW(h, GWLP_USERDATA) as u32;
        match msg {
            // Taskbar click on our (always minimized) button: toggle, but stay minimized.
            WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_RESTORE => {
                toggle(id);
                return LRESULT(0);
            }
            WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_MINIMIZE => {
                minimize_ws(id);
                return LRESULT(0);
            }
            // Something restored us without SC_RESTORE (e.g. ShowWindow from the shell).
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize && !SELF_SIZE.get() && IsWindowVisible(h).as_bool() => {
                keep_rep_minimized(h);
                toggle(id);
                return LRESULT(0);
            }
            WM_CLOSE => {
                close_ws(id);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

const MENU_LAUNCH_SAVED: usize = 9001;
const MENU_PICK_PROGRAM: usize = 9002;

fn pick_for_slot(id: u32, idx: usize, plus: HWND) {
    let members: Vec<isize> = MEMBERS.with(|m| m.borrow().clone());
    let remembered = with(|a| a.idx(id).and_then(|i| a.spaces[i].slots.get(idx).and_then(|s| s.spec.clone())))
        .flatten();
    let cands: Vec<HWND> = win::app_windows()
        .into_iter()
        .filter(|h| !members.contains(&(h.0 as isize)) && !win::is_fullscreen(*h) && win::exe_name(*h) != "ztile.exe")
        .collect();
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        if let Some(s) = &remembered {
            let label = win::wide(&format!("▶  Start {}", s.label()));
            let _ = AppendMenuW(menu, MF_STRING, MENU_LAUNCH_SAVED, PCWSTR(label.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        }
        let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("Put an open window here:"));
        for (i, c) in cands.iter().enumerate() {
            let mut t = win::title(*c);
            if t.chars().count() > 60 {
                t = t.chars().take(57).collect::<String>() + "...";
            }
            let label = win::wide(&format!("{t}\t{}", win::exe_name(*c)));
            let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(label.as_ptr()));
        }
        if cands.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("(no other windows open)"));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_PICK_PROGRAM, w!("Choose a program to start..."));
        let pt = cursor();
        let _ = SetForegroundWindow(plus);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, plus, None);
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    };
    // The "+" window has focus and may get hidden; park focus on our (minimized)
    // representative so Windows doesn't hand it to a member.
    if let Some(rep) = with(|a| a.idx(id).map(|i| a.spaces[i].rep)).flatten() {
        unsafe {
            let _ = SetForegroundWindow(rep);
        }
    }
    match cmd {
        0 => {}
        MENU_LAUNCH_SAVED => {
            if let Some(s) = remembered {
                launch_into(id, idx, s);
            }
        }
        MENU_PICK_PROGRAM => {
            if let Some(spec) = apps::pick_program(plus) {
                with(|a| {
                    if let Some(i) = a.idx(id) {
                        a.spaces[i].slots[idx].spec = Some(spec.clone());
                    }
                });
                persist(id);
                launch_into(id, idx, spec);
            }
        }
        n if n <= cands.len() => {
            let h = cands[n - 1];
            let ok = with(|a| {
                let wi = a.idx(id)?;
                if a.spaces[wi].slots[idx].hwnd.is_some() {
                    return None;
                }
                let tb = a.taskbar.clone();
                adopt(&tb, &mut a.spaces[wi], idx, h);
                sync_members(a);
                Some(())
            })
            .flatten()
            .is_some();
            if ok {
                refresh_icon(id);
                persist(id);
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn plus_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(h, &mut rc);
                let bg = CreateSolidBrush(COLORREF(0x00202020));
                FillRect(dc, &rc, bg);
                let _ = DeleteObject(bg.into());
                let edge = CreateSolidBrush(COLORREF(0x00404040));
                FrameRect(dc, &rc, edge);
                let _ = DeleteObject(edge.into());
                let size = ((rc.right - rc.left).min(rc.bottom - rc.top) / 3).max(16);
                let font = CreateFontW(
                    size,
                    0,
                    0,
                    0,
                    FW_LIGHT.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    0,
                    w!("Segoe UI"),
                );
                let old = SelectObject(dc, font.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00808080));
                let mut txt: Vec<u16> = "+".encode_utf16().collect();
                DrawTextW(dc, &mut txt, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
                let _ = EndPaint(h, &ps);
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let v = GetWindowLongPtrW(h, GWLP_USERDATA);
                pick_for_slot((v >> 16) as u32, (v & 0xFFFF) as usize, h);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

unsafe extern "system" fn overlay_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_PAINT {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let fill = CreateSolidBrush(COLORREF(OVERLAY_COLOR.get()));
            FillRect(dc, &rc, fill);
            let _ = DeleteObject(fill.into());
            // brighter frame so the target cell reads clearly
            let edge = CreateSolidBrush(COLORREF(0x00FFFFFF));
            for k in 0..3 {
                let r = RECT { left: rc.left + k, top: rc.top + k, right: rc.right - k, bottom: rc.bottom - k };
                FrameRect(dc, &r, edge);
            }
            let _ = DeleteObject(edge.into());
            let _ = EndPaint(h, &ps);
            return LRESULT(0);
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
