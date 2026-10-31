//! Phase 0 spike: one live group of windows tiled on a monitor.
//!
//! - member windows lose their own taskbar buttons (ITaskbarList::DeleteTab)
//! - one invisible "representative" window owns a taskbar button with a composite icon
//! - clicking that button toggles the whole group (front <-> minimized)
//! - drag a member onto another cell = swap, outside the grid = detach, small move = snap back
//! - empty cells show a "+" window; clicking it lists open windows to attach

use crate::layout::{self, NRect};
use crate::{icon, win};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

const WM_ZT_EVENT: u32 = WM_APP + 1;
/// Ignore drags shorter than this (px): they snap back instead of swapping.
const SWAP_MIN_MOVE: i32 = 48;

/// Windows the taskbar/shell puts in the foreground while you click it.
const SHELL_FG_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "TaskListThumbnailWnd",
    "XamlExplorerHostIslandWindow",
    "Windows.UI.Core.CoreWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
];

struct Slot {
    hwnd: Option<HWND>,
    old_style: Option<u32>,
    plus: HWND,
}

struct Group {
    rep: HWND,
    area: RECT,
    ncells: Vec<NRect>,
    cells: Vec<RECT>,
    slots: Vec<Slot>,
    taskbar: Option<ITaskbarList>,
    borderless: bool,
    minimized: bool,
    drag_start: Option<(HWND, RECT)>,
    icons: Vec<HICON>,
    hooks: Vec<HWINEVENTHOOK>,
    msg_taskbar_created: u32,
}

impl Group {
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
    /// Ground truth instead of tracked state: is the topmost normal window
    /// over our area one of our members (and none of them minimized)?
    fn on_top(&self) -> bool {
        let members = self.members();
        if members.is_empty() || members.iter().any(|(_, h)| win::is_minimized(*h)) {
            return false;
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

thread_local! {
    static G: RefCell<Option<Group>> = const { RefCell::new(None) };
    static MEMBERS: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
    /// true while *we* resize/minimize the representative window
    static SELF_SIZE: Cell<bool> = const { Cell::new(false) };
    static LAST_TOGGLE: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}
static REP: AtomicIsize = AtomicIsize::new(0);
static DONE: AtomicBool = AtomicBool::new(false);

/// Run `f` on the group state. Returns None if the state is busy (re-entrant call).
fn with<R>(f: impl FnOnce(&mut Group) -> R) -> Option<R> {
    G.with(|g| match g.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
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

pub struct Options {
    pub layout: Option<String>,
    pub monitor: usize,
    pub gap: i32,
    pub borderless: bool,
    pub specs: Vec<String>,
}

pub fn resolve(specs: &[String]) -> Vec<Option<HWND>> {
    let all = win::app_windows();
    let mut used: Vec<HWND> = Vec::new();
    specs
        .iter()
        .map(|s| {
            if s == "+" || s == "_" {
                return None;
            }
            if let Some(hex) = s.strip_prefix("0x") {
                if let Ok(v) = isize::from_str_radix(hex, 16) {
                    let h = HWND(v as *mut _);
                    if win::is_alive(h) {
                        used.push(h);
                        return Some(h);
                    }
                }
            }
            let q = s.to_lowercase();
            let found = all.iter().copied().find(|h| {
                !used.contains(h)
                    && (win::title(*h).to_lowercase().contains(&q) || win::exe_name(*h).to_lowercase().contains(&q))
            });
            match found {
                Some(h) => {
                    used.push(h);
                    Some(h)
                }
                None => {
                    eprintln!("  ! pencere bulunamadı: \"{s}\" (hücre boş kalacak)");
                    None
                }
            }
        })
        .collect()
}

pub fn run(opt: Options) {
    let mons = win::monitors();
    let Some(mon) = mons.get(opt.monitor).copied() else {
        eprintln!("Monitör {} yok ({} monitör bulundu).", opt.monitor, mons.len());
        return;
    };
    let lname = opt.layout.clone().unwrap_or_else(|| layout::default_for(opt.specs.len()).to_string());
    let Some(ncells) = layout::preset(&lname) else {
        eprintln!("Bilinmeyen dizilim: {lname}");
        return;
    };
    let mut hwnds = resolve(&opt.specs);
    if hwnds.len() > ncells.len() {
        eprintln!("  ! {} pencere verildi ama \"{lname}\" {} hücreli; fazlası yok sayıldı.", hwnds.len(), ncells.len());
    }
    hwnds.resize(ncells.len(), None);
    let cells = layout::to_pixels(&ncells, mon.work, opt.gap);

    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(rep_proc),
            hInstance: hinst,
            lpszClassName: w!("ZTileGroup"),
            hCursor: cursor,
            ..Default::default()
        });
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(plus_proc),
            hInstance: hinst,
            lpszClassName: w!("ZTilePlus"),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            ..Default::default()
        });

        let Ok(rep) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_APPWINDOW,
            w!("ZTileGroup"),
            w!("ZTile"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            mon.work.left,
            mon.work.top,
            1,
            1,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            eprintln!("Temsilci pencere oluşturulamadı.");
            return;
        };
        let _ = SetLayeredWindowAttributes(rep, COLORREF(0), 0, LWA_ALPHA);
        REP.store(rep.0 as isize, Ordering::SeqCst);

        let mut slots = Vec::new();
        for (i, c) in cells.iter().enumerate() {
            let plus = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("ZTilePlus"),
                w!(""),
                WS_POPUP,
                c.left,
                c.top,
                c.right - c.left,
                c.bottom - c.top,
                None, // not owned by rep: rep stays minimized, which would hide owned windows
                None,
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            SetWindowLongPtrW(plus, GWLP_USERDATA, i as isize);
            slots.push(Slot { hwnd: hwnds[i], old_style: None, plus });
        }

        let tb = taskbar();
        if tb.is_none() {
            eprintln!("  ! ITaskbarList alınamadı; taskbar butonları gizlenemeyecek.");
        }
        let g = Group {
            rep,
            area: mon.work,
            ncells,
            cells,
            slots,
            taskbar: tb,
            borderless: opt.borderless,
            minimized: false,
            drag_start: None,
            icons: Vec::new(),
            hooks: Vec::new(),
            msg_taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        };
        G.with(|cell| *cell.borrow_mut() = Some(g));

        keep_rep_minimized(rep);
        let members = with(|g| {
            for (i, h) in g.members() {
                adopt(g, i, h);
            }
            for s in &g.slots {
                if s.hwnd.is_none() {
                    let _ = ShowWindow(s.plus, SW_SHOWNOACTIVATE);
                }
            }
            g.members()
        })
        .unwrap_or_default();
        refresh_icon();
        // Bring them up but never focus them: a focused chat app marks messages as read.
        for (_, h) in &members {
            win::raise_no_activate(*h);
        }

        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZESTART),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        ];
        let hooks: Vec<HWINEVENTHOOK> =
            ranges.iter().map(|&(a, b)| SetWinEventHook(a, b, None, Some(hook), 0, 0, flags)).collect();
        with(|g| g.hooks = hooks);

        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);

        println!("Grup kuruldu: \"{lname}\", monitör {}. Taskbar'daki birleşik simgeyi dene.", opt.monitor);
        println!("Bitirmek için: Ctrl+C ya da taskbar simgesine sağ tık > Pencereyi kapat.");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    DONE.store(true, Ordering::SeqCst);
}

/// Take ownership of a window for slot `i`: hide its taskbar button, place it.
fn adopt(g: &mut Group, i: usize, h: HWND) {
    g.slots[i].hwnd = Some(h);
    if let Some(tb) = &g.taskbar {
        unsafe {
            let _ = tb.DeleteTab(h);
        }
    }
    // Borderless only works for classic system title bars. Apps that draw their
    // own (Chromium/Electron) keep theirs: their buttons live up there.
    if g.borderless && g.slots[i].old_style.is_none() && win::native_caption_height(h) > 4 {
        win::set_border_hidden(h, true);
        g.slots[i].old_style = Some(win::make_borderless(h));
    }
    g.place_slot(i);
    win::set_square_corners(h, true);
    let (want, got) = (g.cells[i], win::frame_rect(h));
    if got != want {
        println!(
            "  ~ {}: hedef {:?} sonuç {:?} (uygulama boyut dayatıyor olabilir)",
            win::exe_name(h),
            (want.left, want.top, want.right, want.bottom),
            (got.left, got.top, got.right, got.bottom)
        );
    }
    unsafe {
        let _ = ShowWindow(g.slots[i].plus, SW_HIDE);
    }
}

/// Release slot `i`. `give_back_tab`: the window lives on, so return its taskbar button.
fn release(g: &mut Group, i: usize, give_back_tab: bool) {
    let Some(h) = g.slots[i].hwnd.take() else { return };
    let old_style = g.slots[i].old_style.take();
    if win::is_alive(h) {
        if let Some(st) = old_style {
            win::restore_style(h, st);
            win::set_border_hidden(h, false);
        }
        win::set_square_corners(h, false);
    }
    if give_back_tab && win::is_alive(h) {
        if let Some(tb) = &g.taskbar {
            unsafe {
                let _ = tb.AddTab(h);
            }
        }
    }
    if !g.minimized {
        let p = g.slots[i].plus;
        unsafe {
            let _ = ShowWindow(p, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(p), None, true);
        }
        win::raise_no_activate(p);
    }
}

fn refresh_icon() {
    let Some((rep, ncells, hw, old)) = with(|g| {
        let hw: Vec<Option<HWND>> = g.slots.iter().map(|s| s.hwnd).collect();
        (g.rep, g.ncells.clone(), hw, std::mem::take(&mut g.icons))
    }) else {
        return;
    };
    MEMBERS.with(|m| *m.borrow_mut() = hw.iter().flatten().map(|h| h.0 as isize).collect());

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
        let t = win::wide(&format!("ZTile · {}", names.join(" | ")));
        let _ = SetWindowTextW(rep, PCWSTR(t.as_ptr()));
        with(|g| g.icons = vec![big, small]);
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
fn toggle() {
    let now = std::time::Instant::now();
    // SC_RESTORE and the resulting WM_SIZE can both arrive for one click.
    if LAST_TOGGLE.get().is_some_and(|t| now.duration_since(t).as_millis() < 300) {
        return;
    }
    LAST_TOGGLE.set(Some(now));
    let on_top = with(|g| g.on_top()).unwrap_or(false);
    if on_top { minimize_group() } else { restore_group() }
}

fn minimize_group() {
    let Some((rep, members, plus)) = with(|g| {
        g.minimized = true;
        let plus: Vec<HWND> = g.slots.iter().map(|s| s.plus).collect();
        (g.rep, g.members(), plus)
    }) else {
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

/// Show the group on top without activating any member
/// (a focused chat app would send read receipts).
fn restore_group() {
    let Some((members, plus)) = with(|g| {
        g.minimized = false;
        let plus: Vec<HWND> = g.slots.iter().filter(|s| s.hwnd.is_none()).map(|s| s.plus).collect();
        for (i, h) in g.members() {
            if win::is_minimized(h) {
                unsafe {
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
            }
            g.place_slot(i);
        }
        (g.members(), plus)
    }) else {
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

fn on_drag_end(h: HWND) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    let changed = with(|g| {
        let Some(i) = g.slot_of(h) else { return false };
        let start = g.drag_start.take().filter(|(sh, _)| *sh == h).map(|(_, r)| r);
        let fr = win::frame_rect(h);
        let (resized, moved) = match start {
            Some(s) => (
                s.right - s.left != fr.right - fr.left || s.bottom - s.top != fr.bottom - fr.top,
                (fr.left - s.left).abs() + (fr.top - s.top).abs(),
            ),
            None => (false, i32::MAX),
        };
        if resized {
            g.place_slot(i); // Phase 7: resize neighbours instead
            return false;
        }
        let target = g.cells.iter().position(|c| win::contains(c, pt));
        match target {
            Some(j) if j != i && moved >= SWAP_MIN_MOVE => {
                let other = g.slots[j].hwnd;
                g.slots.swap(i, j);
                // plus windows belong to cells, not to apps: swap them back
                let (pi, pj) = (g.slots[i].plus, g.slots[j].plus);
                g.slots[i].plus = pj;
                g.slots[j].plus = pi;
                g.place_slot(j);
                match other {
                    Some(_) => g.place_slot(i),
                    None => unsafe {
                        let _ = ShowWindow(g.slots[i].plus, SW_SHOWNOACTIVATE);
                        let _ = ShowWindow(g.slots[j].plus, SW_HIDE);
                    },
                }
                true
            }
            None if !win::contains(&g.area, pt) => {
                release(g, i, true);
                println!("  - \"{}\" gruptan ayrıldı", win::exe_name(h));
                true
            }
            _ => {
                g.place_slot(i);
                false
            }
        }
    })
    .unwrap_or(false);
    if changed {
        refresh_icon();
    }
}

fn on_event(ev: u32, h: HWND) {
    match ev {
        EVENT_SYSTEM_FOREGROUND => {
            let c = win::class(h);
            if SHELL_FG_CLASSES.contains(&c.as_str()) {
                return;
            }
            with(|g| {
                if h == g.rep {
                    return;
                }
                if g.slot_of(h).is_some() {
                    g.minimized = false;
                }
            });
        }
        EVENT_SYSTEM_MOVESIZESTART => {
            with(|g| {
                if g.slot_of(h).is_some() {
                    g.drag_start = Some((h, win::frame_rect(h)));
                }
            });
        }
        EVENT_SYSTEM_MOVESIZEEND => on_drag_end(h),
        EVENT_SYSTEM_MINIMIZESTART => {
            // Minimizing one member minimizes the whole group (its own button is gone).
            let go = with(|g| g.slot_of(h).is_some() && !g.minimized).unwrap_or(false);
            if go && win::is_minimized(h) {
                minimize_group();
            }
        }
        EVENT_OBJECT_DESTROY | EVENT_OBJECT_HIDE => {
            let changed = with(|g| match g.slot_of(h) {
                Some(i) => {
                    println!("  - \"{}\" kapandı/gizlendi, hücre {} boş", win::title(h), i + 1);
                    release(g, i, ev == EVENT_OBJECT_HIDE);
                    true
                }
                None => false,
            })
            .unwrap_or(false);
            if changed {
                refresh_icon();
            }
        }
        EVENT_OBJECT_LOCATIONCHANGE => {
            // Only fight maximize (double-click on title etc.); normal moves end in MOVESIZEEND.
            with(|g| {
                if let Some(i) = g.slot_of(h) {
                    if win::is_maximized(h) {
                        g.place_slot(i);
                    }
                }
            });
        }
        _ => {}
    }
}

unsafe extern "system" fn hook(_: HWINEVENTHOOK, ev: u32, h: HWND, obj: i32, child: i32, _: u32, _: u32) {
    if obj != 0 || child != 0 || h.is_invalid() {
        return; // only whole windows (OBJID_WINDOW, CHILDID_SELF)
    }
    if ev == EVENT_OBJECT_LOCATIONCHANGE || ev == EVENT_OBJECT_HIDE {
        // very chatty: drop non-members right here
        let member = MEMBERS.with(|m| m.try_borrow().map(|m| m.contains(&(h.0 as isize))).unwrap_or(true));
        if !member {
            return;
        }
    }
    let rep = HWND(REP.load(Ordering::Relaxed) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(rep), WM_ZT_EVENT, WPARAM(ev as usize), LPARAM(h.0 as isize));
    }
}

unsafe extern "system" fn on_ctrl(_: u32) -> BOOL {
    let rep = HWND(REP.load(Ordering::SeqCst) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(rep), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    // Console close kills us when this returns: give cleanup a moment.
    for _ in 0..50 {
        if DONE.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    true.into()
}

fn shutdown() {
    let Some(mut g) = G.with(|c| c.try_borrow_mut().ok().and_then(|mut c| c.take())) else {
        return;
    };
    for i in 0..g.slots.len() {
        if let Some(h) = g.slots[i].hwnd {
            if win::is_minimized(h) {
                unsafe {
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
            }
        }
        release(&mut g, i, true);
    }
    unsafe {
        for hk in g.hooks.drain(..) {
            let _ = UnhookWinEvent(hk);
        }
        for s in &g.slots {
            let _ = DestroyWindow(s.plus);
        }
        for ic in g.icons.drain(..) {
            let _ = DestroyIcon(ic);
        }
    }
    println!("Grup çözüldü, taskbar butonları geri verildi.");
}

unsafe extern "system" fn rep_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ZT_EVENT => {
                on_event(wp.0 as u32, HWND(lp.0 as *mut _));
                return LRESULT(0);
            }
            // Taskbar click on our (always minimized) button: toggle, but stay minimized.
            WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_RESTORE => {
                toggle();
                return LRESULT(0);
            }
            WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_MINIMIZE => {
                minimize_group();
                return LRESULT(0);
            }
            // Something restored us without SC_RESTORE (e.g. ShowWindow from the shell).
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize && !SELF_SIZE.get() && IsWindowVisible(h).as_bool() => {
                keep_rep_minimized(h);
                toggle();
                return LRESULT(0);
            }
            WM_CLOSE => {
                shutdown();
                let _ = DestroyWindow(h);
                return LRESULT(0);
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        let tb_created = with(|g| g.msg_taskbar_created).unwrap_or(0);
        if msg != 0 && msg == tb_created {
            // Explorer restarted: our DeleteTab calls are forgotten, redo them.
            println!("  * Explorer yeniden başladı, butonlar tekrar gizleniyor");
            with(|g| {
                g.taskbar = taskbar();
                if let Some(tb) = &g.taskbar {
                    for (_, m) in g.members() {
                        let _ = tb.DeleteTab(m);
                    }
                }
            });
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

fn pick_for_slot(idx: usize, plus: HWND) {
    let members: Vec<HWND> = with(|g| g.members().into_iter().map(|m| m.1).collect()).unwrap_or_default();
    let cands: Vec<HWND> = win::app_windows()
        .into_iter()
        .filter(|h| !members.contains(h) && !win::is_fullscreen(*h) && win::exe_name(*h) != "ztile.exe")
        .collect();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("Açık pencerelerden seç:"));
        for (i, c) in cands.iter().enumerate() {
            let mut t = win::title(*c);
            if t.chars().count() > 60 {
                t = t.chars().take(57).collect::<String>() + "...";
            }
            let label = win::wide(&format!("{t}\t{}", win::exe_name(*c)));
            let _ = AppendMenuW(menu, MF_STRING, i + 1, PCWSTR(label.as_ptr()));
        }
        if cands.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("(uygun pencere yok)"));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("Yeni uygulama başlat...  (Faz 5)"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(plus);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, plus, None);
        let _ = DestroyMenu(menu);
        let pick = cmd.0 as usize;
        if pick == 0 || pick > cands.len() {
            return;
        }
        let h = cands[pick - 1];
        // The "+" window has focus and is about to be hidden; hand focus to our
        // representative first so Windows doesn't pass it to a member.
        let rep = with(|g| g.rep).unwrap_or_default();
        let _ = SetForegroundWindow(rep);
        let ok = with(|g| {
            if g.slots[idx].hwnd.is_some() {
                return false;
            }
            adopt(g, idx, h);
            true
        })
        .unwrap_or(false);
        if ok {
            println!("  + \"{}\" hücre {}'e eklendi", win::exe_name(h), idx + 1);
            refresh_icon();
        }
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
                let idx = GetWindowLongPtrW(h, GWLP_USERDATA) as usize;
                pick_for_slot(idx, h);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
