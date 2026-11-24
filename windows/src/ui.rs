//! Main window: saved layouts as cards, and the layout editor.
//!
//! Plain Win32 + GDI, double-buffered, follows the system light/dark theme.
//! The window is destroyed when closed (ZTile goes back to the tray), so the UI
//! costs nothing while you don't look at it.
//!
//! State changes happen inside `with()`; anything that runs a modal loop or calls
//! into other modules (menus, dialogs, opening workspaces) runs outside it.

use crate::apps::{self, AppSpec};
use crate::layout::{self, Dir, Divider, Node};
use crate::store::{MonitorId, SavedLayout, Store};
use crate::{icon, win, workspace};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

const CLASS: PCWSTR = w!("ZTileMain");
const ID_EDIT: usize = 100;
const WM_MOUSELEAVE_: u32 = 0x02A3;
const EM_LIMITTEXT_: u32 = 0x00C5;

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

struct Theme {
    bg: u32,
    surface: u32,
    hover: u32,
    border: u32,
    text: u32,
    sub: u32,
    /// filled buttons (white text on it)
    accent: u32,
    accent_hover: u32,
    /// lines, dots, selection on the background
    accent_fg: u32,
    danger: u32,
    cell: u32,
    cell_empty: u32,
}

fn theme(dark: bool) -> Theme {
    if dark {
        Theme {
            bg: rgb(32, 32, 32),
            surface: rgb(43, 43, 43),
            hover: rgb(55, 55, 55),
            border: rgb(62, 62, 62),
            text: rgb(240, 240, 240),
            sub: rgb(160, 160, 160),
            accent: rgb(0, 103, 192),
            accent_hover: rgb(0, 120, 215),
            accent_fg: rgb(96, 165, 250),
            danger: rgb(255, 99, 99),
            cell: rgb(62, 62, 62),
            cell_empty: rgb(36, 36, 36),
        }
    } else {
        Theme {
            bg: rgb(243, 243, 243),
            surface: rgb(255, 255, 255),
            hover: rgb(234, 234, 234),
            border: rgb(218, 218, 218),
            text: rgb(26, 26, 26),
            sub: rgb(96, 96, 96),
            accent: rgb(0, 103, 192),
            accent_hover: rgb(0, 120, 215),
            accent_fg: rgb(0, 103, 192),
            danger: rgb(196, 43, 28),
            cell: rgb(222, 222, 222),
            cell_empty: rgb(246, 246, 246),
        }
    }
}

fn system_is_dark() -> bool {
    let mut v = 1u32;
    let mut n = 4u32;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut n),
        );
    }
    v == 0
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    New,
    Card(usize),
    Back,
    Monitor(usize),
    Preset(usize),
    Cell(usize),
    Divider(usize),
    Borderless,
    KeepChanges,
    GapMinus,
    GapPlus,
    Cancel,
    Save,
    SaveOpen,
}

impl Hit {
    fn clickable(self) -> bool {
        !matches!(self, Hit::Divider(_))
    }
}

struct Editor {
    /// name of the layout being edited (None = new)
    original: Option<String>,
    tree: Node,
    apps: Vec<Option<AppSpec>>,
    monitor: usize,
    borderless: bool,
    keep_changes: bool,
    gap: i32,
    error: String,
    /// where the preview was last drawn (for divider dragging)
    canvas: RECT,
    dividers: Vec<Divider>,
}

struct Fonts {
    title: HFONT,
    h2: HFONT,
    body: HFONT,
    small: HFONT,
    big: HFONT,
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let f = |size: i32, weight: i32| unsafe {
            CreateFontW(
                -(size * dpi as i32 / 96),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        };
        Fonts { title: f(26, 600), h2: f(15, 600), body: f(14, 400), small: f(12, 400), big: f(34, 300) }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.title, self.h2, self.body, self.small, self.big] {
                let _ = DeleteObject(f.into());
            }
        }
    }
}

struct Ui {
    hwnd: HWND,
    edit: HWND,
    dpi: u32,
    dark: bool,
    th: Theme,
    fonts: Fonts,
    edit_brush: HBRUSH,
    store: Store,
    mons: Vec<win::Monitor>,
    open: Vec<String>,
    /// icon per app (exe|aumid); bool = we own it and must destroy it
    icons: HashMap<String, (HICON, bool)>,
    editor: Option<Editor>,
    hits: Vec<(RECT, Hit)>,
    hover: Option<Hit>,
    dragging: Option<Divider>,
    tracking: bool,
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            for (ic, owned) in self.icons.values() {
                if *owned {
                    let _ = DestroyIcon(*ic);
                }
            }
            let _ = DeleteObject(self.edit_brush.into());
        }
    }
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| match u.try_borrow_mut() {
        Ok(mut u) => u.as_mut().map(f),
        Err(_) => None,
    })
}

fn hwnd() -> Option<HWND> {
    with(|u| u.hwnd)
}

// ---------------------------------------------------------------- public

pub fn show() {
    if let Some(h) = hwnd() {
        unsafe {
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
        }
        refresh();
        return;
    }
    create();
}

/// Reload saved layouts / open state and redraw (no-op when the window is closed).
pub fn refresh() {
    let h = with(|u| {
        u.store = Store::load();
        u.open = workspace::open_names();
        u.hwnd
    });
    if let Some(h) = h {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

pub fn close() {
    if let Some(h) = hwnd() {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

fn create() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        if !REGISTERED.get() {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: hinst,
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            });
            REGISTERED.set(true);
        }
        let dpi = GetDpiForSystem();
        let s = |v: i32| v * dpi as i32 / 96;
        let (w, h) = (s(1000), s(700));
        let work = win::monitors().into_iter().find(|m| m.primary).map(|m| m.work).unwrap_or_default();
        let x = work.left + ((work.right - work.left) - w).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - h).max(0) / 2;
        let Ok(hwnd) =
            CreateWindowExW(Default::default(), CLASS, w!("ZTile"), WS_OVERLAPPEDWINDOW, x, y, w, h, None, None, Some(hinst), None)
        else {
            return;
        };
        let dark = system_is_dark();
        let v = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const BOOL as *const c_void,
            size_of::<BOOL>() as u32,
        );
        let dpi = GetDpiForWindow(hwnd);
        let big = icon::app_icon(32 * dpi as i32 / 96);
        let small = icon::app_icon(16 * dpi as i32 / 96);
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(big.0 as isize)));
        SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(small.0 as isize)));

        let th = theme(dark);
        let ui = Ui {
            hwnd,
            edit: HWND::default(),
            dpi,
            dark,
            edit_brush: CreateSolidBrush(COLORREF(th.surface)),
            th,
            fonts: Fonts::new(dpi),
            store: Store::load(),
            mons: win::monitors(),
            open: workspace::open_names(),
            icons: HashMap::new(),
            editor: None,
            hits: Vec::new(),
            hover: None,
            dragging: None,
            tracking: false,
        };
        UI.with(|u| *u.borrow_mut() = Some(ui));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

// ---------------------------------------------------------------- drawing helpers

fn fill(dc: HDC, r: &RECT, c: u32) {
    unsafe {
        let b = CreateSolidBrush(COLORREF(c));
        FillRect(dc, r, b);
        let _ = DeleteObject(b.into());
    }
}

fn rrect(dc: HDC, r: &RECT, radius: i32, fillc: u32, border: Option<u32>) {
    unsafe {
        let b = CreateSolidBrush(COLORREF(fillc));
        let p = match border {
            Some(c) => CreatePen(PS_SOLID, 1, COLORREF(c)),
            None => CreatePen(PS_NULL, 0, COLORREF(0)),
        };
        let ob = SelectObject(dc, b.into());
        let op = SelectObject(dc, p.into());
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        let _ = DeleteObject(b.into());
        let _ = DeleteObject(p.into());
    }
}

fn circle(dc: HDC, cx: i32, cy: i32, r: i32, c: u32) {
    let rc = RECT { left: cx - r, top: cy - r, right: cx + r + 1, bottom: cy + r + 1 };
    rrect(dc, &rc, r + 1, c, None);
}

fn text(dc: HDC, r: RECT, s: &str, font: HFONT, color: u32, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        let mut r = r;
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(color));
        let mut w: Vec<u16> = s.encode_utf16().collect();
        if !w.is_empty() {
            DrawTextW(dc, &mut w, &mut r, flags | DT_NOPREFIX);
        }
        SelectObject(dc, old);
    }
}

const LEFT: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);
const CENTER: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

fn shrink(r: RECT, d: i32) -> RECT {
    rect(r.left + d, r.top + d, r.right - d, r.bottom - d)
}

/// Largest rect of the given aspect ratio centered in `r`.
fn fit(r: RECT, aspect: f32) -> RECT {
    let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let (fw, fh) = if w / h > aspect { (h * aspect, h) } else { (w, w / aspect) };
    let x = r.left + ((w - fw) / 2.0) as i32;
    let y = r.top + ((h - fh) / 2.0) as i32;
    rect(x, y, x + fw as i32, y + fh as i32)
}

fn aspect_of(w: i32, h: i32) -> f32 {
    if w > 0 && h > 0 { w as f32 / h as f32 } else { 16.0 / 10.0 }
}

fn monitor_label(i: usize, m: &win::Monitor) -> String {
    format!("{} · {}×{}", i + 1, m.rect.right - m.rect.left, m.rect.bottom - m.rect.top)
}

fn mouse(lp: LPARAM) -> POINT {
    POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 }
}

// ---------------------------------------------------------------- painting

impl Ui {
    fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    fn icon_for(&mut self, spec: &AppSpec) -> HICON {
        let key = format!("{}|{}", spec.exe.to_lowercase(), spec.aumid);
        if let Some((ic, _)) = self.icons.get(&key) {
            return *ic;
        }
        // A running window has the best icon (browser PWAs show their own, not Brave's).
        let found = apps::match_windows(&[Some(spec.clone())]).into_iter().next().flatten();
        let entry = match found {
            Some(h) => (icon::window_icon(h), false),
            None => {
                let ic = icon::exe_icon(&spec.exe);
                if ic.is_invalid() {
                    (unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() }, false)
                } else {
                    (ic, true)
                }
            }
        };
        self.icons.insert(key, entry);
        entry.0
    }

    fn hit(&mut self, r: RECT, h: Hit) {
        self.hits.push((r, h));
    }

    fn hovered(&self, h: Hit) -> bool {
        self.hover == Some(h)
    }

    fn button(&mut self, dc: HDC, r: RECT, label: &str, h: Hit, primary: bool) {
        let th = &self.th;
        let (bg, fg, border) = if primary {
            (if self.hover == Some(h) { th.accent_hover } else { th.accent }, rgb(255, 255, 255), None)
        } else {
            (if self.hover == Some(h) { th.hover } else { th.surface }, th.text, Some(th.border))
        };
        rrect(dc, &r, self.s(6), bg, border);
        text(dc, r, label, self.fonts.body, fg, CENTER);
        self.hit(r, h);
    }

    /// On/off switch with a label; the whole row is clickable.
    fn switch(&mut self, dc: HDC, x: i32, y: i32, on: bool, label: &str, h: Hit) {
        let sw = rect(x, y + self.s(4), x + self.s(40), y + self.s(24));
        rrect(dc, &sw, self.s(10), if on { self.th.accent } else { self.th.cell }, None);
        let kx = if on { sw.right - self.s(10) } else { sw.left + self.s(10) };
        circle(dc, kx, (sw.top + sw.bottom) / 2, self.s(7), rgb(255, 255, 255));
        let lab = rect(sw.right + self.s(10), y, sw.right + self.s(520), y + self.s(28));
        text(dc, lab, label, self.fonts.body, self.th.text, LEFT);
        self.hit(rect(sw.left, y, lab.right, lab.bottom), h);
    }

    /// Draw a layout's cells into `area`. Returns the cell rects.
    fn draw_layout(
        &mut self,
        dc: HDC,
        area: RECT,
        tree: &Node,
        apps: Option<&[Option<AppSpec>]>,
        gap: i32,
        big: bool,
    ) -> Vec<RECT> {
        let cells = layout::to_pixels(&tree.cells(), area, gap);
        for (i, c) in cells.iter().enumerate() {
            let app = apps.and_then(|a| a.get(i)).cloned().flatten();
            let hot = big && self.hovered(Hit::Cell(i));
            let col = if hot {
                self.th.hover
            } else if app.is_some() || apps.is_none() {
                // apps == None: a preset thumbnail, cells should read clearly
                self.th.cell
            } else {
                self.th.cell_empty
            };
            rrect(dc, c, self.s(if big { 8 } else { 3 }), col, big.then_some(self.th.border));
            let (cw, ch) = (c.right - c.left, c.bottom - c.top);
            match app {
                Some(spec) => {
                    let size = ((cw.min(ch) as f32) * if big { 0.28 } else { 0.5 }) as i32;
                    let size = size.clamp(self.s(10), self.s(48));
                    let ic = self.icon_for(&spec);
                    let label_h = if big { self.s(22) } else { 0 };
                    let x = c.left + (cw - size) / 2;
                    let y = c.top + (ch - size - label_h) / 2;
                    unsafe {
                        let _ = DrawIconEx(dc, x, y, ic, size, size, 0, None, DI_NORMAL);
                    }
                    if big {
                        let name = spec.label();
                        let r = rect(c.left + self.s(8), y + size + self.s(4), c.right - self.s(8), y + size + label_h);
                        text(dc, r, &name, self.fonts.small, self.th.text, CENTER);
                    }
                }
                None if big => {
                    let mid = c.top + ch / 2;
                    let r = rect(c.left, mid - self.s(34), c.right, mid + self.s(6));
                    text(dc, r, "+", self.fonts.big, self.th.sub, CENTER);
                    let r = rect(c.left, mid + self.s(6), c.right, mid + self.s(28));
                    text(dc, r, "Choose an app", self.fonts.small, self.th.sub, CENTER);
                }
                None => {}
            }
        }
        cells
    }

    fn paint(&mut self, dc: HDC, rc: RECT) {
        self.hits.clear();
        fill(dc, &rc, self.th.bg);
        if self.editor.is_some() {
            self.paint_editor(dc, rc);
        } else {
            self.paint_home(dc, rc);
        }
    }

    fn paint_home(&mut self, dc: HDC, rc: RECT) {
        let pad = self.s(28);
        let th_text = self.th.text;
        text(dc, rect(pad, pad, rc.right - pad, pad + self.s(38)), "ZTile", self.fonts.title, th_text, LEFT);
        text(
            dc,
            rect(pad, pad + self.s(38), rc.right - pad, pad + self.s(60)),
            "Saved layouts",
            self.fonts.body,
            self.th.sub,
            LEFT,
        );
        let nb = rect(rc.right - pad - self.s(170), pad + self.s(6), rc.right - pad, pad + self.s(44));
        self.button(dc, nb, "+  New layout", Hit::New, true);

        let top = pad + self.s(84);
        let (cw, ch, gap) = (self.s(250), self.s(216), self.s(16));
        let cols = ((rc.right - 2 * pad + gap) / (cw + gap)).max(1);
        let layouts = self.store.layouts.clone();
        if layouts.is_empty() {
            let mid = (top + rc.bottom) / 2;
            text(
                dc,
                rect(pad, mid - self.s(30), rc.right - pad, mid),
                "No saved layouts yet",
                self.fonts.h2,
                self.th.text,
                CENTER,
            );
            text(
                dc,
                rect(pad, mid, rc.right - pad, mid + self.s(26)),
                "Create your first one with \u{201C}New layout\u{201D} in the top right.",
                self.fonts.body,
                self.th.sub,
                CENTER,
            );
        }
        for (i, l) in layouts.iter().enumerate() {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            let x = pad + col * (cw + gap);
            let y = top + row * (ch + gap);
            let card = rect(x, y, x + cw, y + ch);
            if card.top > rc.bottom {
                break;
            }
            let hot = self.hovered(Hit::Card(i));
            rrect(dc, &card, self.s(10), if hot { self.th.hover } else { self.th.surface }, Some(self.th.border));
            let pbox = rect(card.left + self.s(14), card.top + self.s(14), card.right - self.s(14), card.top + self.s(142));
            let screen = fit(pbox, aspect_of(l.monitor.width, l.monitor.height));
            rrect(dc, &screen, self.s(6), self.th.bg, None);
            let g = self.s(3);
            self.draw_layout(dc, shrink(screen, self.s(4)), &l.tree, Some(&l.apps), g, false);

            let name_r = rect(card.left + self.s(16), pbox.bottom + self.s(10), card.right - self.s(16), pbox.bottom + self.s(34));
            text(dc, name_r, &l.name, self.fonts.h2, self.th.text, LEFT);
            let napps = l.apps.iter().flatten().count();
            let mon = l
                .monitor
                .find(&self.mons)
                .and_then(|m| self.mons.iter().position(|x| x.device == m.device))
                .map(|i| format!("Display {}", i + 1))
                .unwrap_or_default();
            let info_r = rect(card.left + self.s(16), name_r.bottom, card.right - self.s(16), name_r.bottom + self.s(22));
            let apps_txt = if napps == 1 { "1 app".to_string() } else { format!("{napps} apps") };
            text(dc, info_r, &format!("{apps_txt} · {mon}"), self.fonts.small, self.th.sub, LEFT);
            if self.open.iter().any(|n| n.eq_ignore_ascii_case(&l.name)) {
                let r = rect(info_r.right - self.s(48), info_r.top, info_r.right, info_r.bottom);
                text(dc, r, "open", self.fonts.small, self.th.accent_fg, DRAW_TEXT_FORMAT(DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0));
                circle(dc, r.left + self.s(6), (r.top + r.bottom) / 2, self.s(3), self.th.accent_fg);
            }
            self.hit(card, Hit::Card(i));
        }
        let foot = rect(pad, rc.bottom - pad, rc.right - pad, rc.bottom - self.s(8));
        text(dc, foot, "Click a card to open it  ·  Right-click to edit, close or delete", self.fonts.small, self.th.sub, LEFT);
    }

    /// Fixed position of the name box (the EDIT child lives inside it).
    fn name_box(&self) -> RECT {
        let pad = self.s(28);
        let y = pad + self.s(56);
        rect(pad + self.s(80), y, pad + self.s(380), y + self.s(36))
    }

    fn paint_editor(&mut self, dc: HDC, rc: RECT) {
        let pad = self.s(28);
        let ed = self.editor.as_ref().unwrap();
        let (tree, apps, mon_idx, borderless, keep, gapv, err, is_new) = (
            ed.tree.clone(),
            ed.apps.clone(),
            ed.monitor,
            ed.borderless,
            ed.keep_changes,
            ed.gap,
            ed.error.clone(),
            ed.original.is_none(),
        );

        // header
        let back = rect(pad - self.s(8), pad, pad + self.s(84), pad + self.s(36));
        if self.hovered(Hit::Back) {
            rrect(dc, &back, self.s(6), self.th.hover, None);
        }
        text(dc, back, "←  Back", self.fonts.body, self.th.text, CENTER);
        self.hit(back, Hit::Back);
        let title = if is_new { "New layout" } else { "Edit layout" };
        text(dc, rect(pad + self.s(100), pad, rc.right - pad, pad + self.s(36)), title, self.fonts.h2, self.th.text, LEFT);

        // name + monitor
        let nb = self.name_box();
        text(dc, rect(pad, nb.top, nb.left, nb.bottom), "Name", self.fonts.body, self.th.sub, LEFT);
        rrect(dc, &nb, self.s(6), self.th.surface, Some(self.th.border));
        let mut x = nb.right + self.s(36);
        text(dc, rect(x, nb.top, x + self.s(64), nb.bottom), "Display", self.fonts.body, self.th.sub, LEFT);
        x += self.s(70);
        for (i, m) in self.mons.clone().iter().enumerate() {
            let r = rect(x, nb.top, x + self.s(150), nb.bottom);
            let sel = i == mon_idx;
            let bg = if sel {
                self.th.accent
            } else if self.hovered(Hit::Monitor(i)) {
                self.th.hover
            } else {
                self.th.surface
            };
            rrect(dc, &r, self.s(6), bg, (!sel).then_some(self.th.border));
            let label = format!("{}{}", monitor_label(i, m), if m.primary { " ★" } else { "" });
            text(dc, r, &label, self.fonts.body, if sel { rgb(255, 255, 255) } else { self.th.text }, CENTER);
            self.hit(r, Hit::Monitor(i));
            x = r.right + self.s(8);
        }

        // presets
        let py = nb.bottom + self.s(18);
        text(dc, rect(pad, py, nb.left, py + self.s(42)), "Presets", self.fonts.body, self.th.sub, LEFT);
        let mut x = nb.left;
        for (i, p) in layout::PRESETS.iter().enumerate() {
            let r = rect(x, py, x + self.s(64), py + self.s(42));
            let bg = if self.hovered(Hit::Preset(i)) { self.th.hover } else { self.th.surface };
            rrect(dc, &r, self.s(6), bg, Some(self.th.border));
            let t = layout::preset(p).unwrap();
            let g = self.s(2);
            self.draw_layout(dc, shrink(r, self.s(6)), &t, None, g, false);
            self.hit(r, Hit::Preset(i));
            x = r.right + self.s(8);
        }

        // canvas
        let top = py + self.s(60);
        let bottom = rc.bottom - pad - self.s(146);
        let mon = self.mons.get(mon_idx).cloned();
        let aspect = mon.map(|m| aspect_of(m.rect.right - m.rect.left, m.rect.bottom - m.rect.top)).unwrap_or(1.6);
        let screen = fit(rect(pad, top, rc.right - pad, bottom.max(top + self.s(80))), aspect);
        rrect(dc, &screen, self.s(10), self.th.surface, Some(self.th.border));
        let canvas = shrink(screen, self.s(10));
        let dividers = tree.dividers();
        let (cw, chh) = ((canvas.right - canvas.left) as f32, (canvas.bottom - canvas.top) as f32);
        let half = self.s(6);
        // dividers first: they win the hit test over the cells below them
        for (k, d) in dividers.iter().enumerate() {
            let r = match d.dir {
                Dir::Cols => {
                    let xx = canvas.left + (d.pos * cw) as i32;
                    rect(xx - half, canvas.top + (d.parent.y0 * chh) as i32, xx + half, canvas.top + (d.parent.y1 * chh) as i32)
                }
                Dir::Rows => {
                    let yy = canvas.top + (d.pos * chh) as i32;
                    rect(canvas.left + (d.parent.x0 * cw) as i32, yy - half, canvas.left + (d.parent.x1 * cw) as i32, yy + half)
                }
            };
            self.hit(r, Hit::Divider(k));
        }
        let g = self.s(6);
        let cells = self.draw_layout(dc, canvas, &tree, Some(&apps), g, true);
        for (k, d) in dividers.iter().enumerate() {
            let active = self.hovered(Hit::Divider(k)) || self.dragging.as_ref() == Some(d);
            if active {
                let r = self.hits.iter().find(|(_, h)| *h == Hit::Divider(k)).map(|(r, _)| *r).unwrap();
                let line = match d.dir {
                    Dir::Cols => rect((r.left + r.right) / 2 - self.s(2), r.top, (r.left + r.right) / 2 + self.s(2), r.bottom),
                    Dir::Rows => rect(r.left, (r.top + r.bottom) / 2 - self.s(2), r.right, (r.top + r.bottom) / 2 + self.s(2)),
                };
                rrect(dc, &line, self.s(2), self.th.accent_fg, None);
            }
        }
        for (i, c) in cells.iter().enumerate() {
            self.hit(*c, Hit::Cell(i));
        }
        if let Some(e) = self.editor.as_mut() {
            e.canvas = canvas;
            e.dividers = dividers;
        }

        // options
        let oy = screen.bottom + self.s(18);
        self.switch(dc, pad, oy, borderless, "Hide title bars (apps with a classic title bar only)", Hit::Borderless);
        self.switch(
            dc,
            pad,
            oy + self.s(34),
            keep,
            "Save changes made in the open group (dragging, +) to this layout",
            Hit::KeepChanges,
        );

        let gx = rc.right - pad - self.s(250);
        text(dc, rect(gx, oy, gx + self.s(110), oy + self.s(28)), "Gap", self.fonts.body, self.th.text, LEFT);
        let minus = rect(gx + self.s(116), oy, gx + self.s(146), oy + self.s(28));
        self.button(dc, minus, "−", Hit::GapMinus, false);
        text(
            dc,
            rect(minus.right, oy, minus.right + self.s(70), oy + self.s(28)),
            &format!("{gapv} px"),
            self.fonts.body,
            self.th.text,
            CENTER,
        );
        let plus = rect(minus.right + self.s(70), oy, minus.right + self.s(100), oy + self.s(28));
        self.button(dc, plus, "+", Hit::GapPlus, false);

        // bottom bar
        let by = rc.bottom - pad - self.s(38);
        let so = rect(rc.right - pad - self.s(150), by, rc.right - pad, by + self.s(38));
        self.button(dc, so, "Save and open", Hit::SaveOpen, true);
        let sv = rect(so.left - self.s(110), by, so.left - self.s(10), by + self.s(38));
        self.button(dc, sv, "Save", Hit::Save, false);
        let cc = rect(sv.left - self.s(100), by, sv.left - self.s(10), by + self.s(38));
        self.button(dc, cc, "Cancel", Hit::Cancel, false);
        if !err.is_empty() {
            text(dc, rect(pad, by, cc.left - self.s(16), by + self.s(38)), &err, self.fonts.body, self.th.danger, LEFT);
        } else {
            text(
                dc,
                rect(pad, by, cc.left - self.s(16), by + self.s(38)),
                "Click a cell to choose an app, split or remove it  ·  Drag a divider to resize",
                self.fonts.small,
                self.th.sub,
                LEFT,
            );
        }
    }

    fn hit_at(&self, p: POINT) -> Option<Hit> {
        self.hits.iter().find(|(r, _)| win::contains(r, p)).map(|(_, h)| *h)
    }

    // ------------------------------------------------------------ editor state

    fn enter_editor(&mut self, l: Option<&SavedLayout>) {
        self.mons = win::monitors();
        let ed = match l {
            Some(l) => Editor {
                original: Some(l.name.clone()),
                tree: l.tree.clone(),
                apps: l.apps.clone(),
                monitor: l
                    .monitor
                    .find(&self.mons)
                    .and_then(|m| self.mons.iter().position(|x| x.device == m.device))
                    .unwrap_or(0),
                borderless: l.borderless,
                keep_changes: l.keep_changes,
                gap: l.gap,
                error: String::new(),
                canvas: RECT::default(),
                dividers: Vec::new(),
            },
            None => Editor {
                original: None,
                tree: layout::preset("2x2").unwrap(),
                apps: vec![None; 4],
                monitor: self.mons.iter().position(|m| m.primary).unwrap_or(0),
                borderless: false,
                keep_changes: false,
                gap: 0,
                error: String::new(),
                canvas: RECT::default(),
                dividers: Vec::new(),
            },
        };
        let name = l.map(|l| l.name.clone()).unwrap_or_default();
        self.editor = Some(ed);
        self.hover = None;
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let nb = self.name_box();
            let (px, py) = (self.s(10), self.s(8));
            let nw = win::wide(&name);
            self.edit = CreateWindowExW(
                Default::default(),
                w!("EDIT"),
                PCWSTR(nw.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                nb.left + px,
                nb.top + py,
                nb.right - nb.left - 2 * px,
                nb.bottom - nb.top - 2 * py,
                Some(self.hwnd),
                Some(HMENU(ID_EDIT as *mut c_void)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(self.fonts.body.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.edit, EM_LIMITTEXT_, Some(WPARAM(60)), Some(LPARAM(0)));
            let _ = SetFocus(Some(self.edit));
        }
    }

    fn leave_editor(&mut self) {
        self.editor = None;
        self.hover = None;
        self.dragging = None;
        if !self.edit.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.edit);
            }
            self.edit = HWND::default();
        }
        self.store = Store::load();
        self.open = workspace::open_names();
    }

    fn name_text(&self) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetWindowTextW(self.edit, &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string()
    }

    /// Validate and write the editor's layout. Returns (layout, previous name).
    fn save_editor(&mut self) -> Result<(SavedLayout, Option<String>), String> {
        let name = self.name_text();
        let ed = self.editor.as_ref().ok_or_else(String::new)?;
        if name.is_empty() {
            return Err("Give the layout a name.".into());
        }
        let mut st = Store::load();
        let renamed = ed.original.as_ref().is_none_or(|o| !o.eq_ignore_ascii_case(&name));
        if renamed && st.get(&name).is_some() {
            return Err(format!("A layout named \u{201C}{name}\u{201D} already exists."));
        }
        let mon = self.mons.get(ed.monitor).ok_or("That display isn't connected.")?;
        let l = SavedLayout {
            name: name.clone(),
            monitor: MonitorId::of(mon),
            tree: ed.tree.clone(),
            gap: ed.gap,
            borderless: ed.borderless,
            keep_changes: ed.keep_changes,
            apps: ed.apps.clone(),
        };
        if let Some(o) = &ed.original {
            if renamed {
                st.remove(o);
            }
        }
        st.put(l.clone());
        st.save().map_err(|e| format!("Couldn't save: {e}"))?;
        Ok((l, ed.original.clone()))
    }
}

// ---------------------------------------------------------------- actions (outside the borrow)

enum Action {
    None,
    Redraw,
    OpenLayout(SavedLayout),
    CardMenu(usize),
    CellMenu(usize),
    Save(bool),
}

fn redraw() {
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn run(a: Action) {
    match a {
        Action::None => {}
        Action::Redraw => redraw(),
        Action::OpenLayout(l) => {
            workspace::open_saved(&l, None);
            close(); // back to the tray; the group is what you wanted to see
        }
        Action::CardMenu(i) => card_menu(i),
        Action::CellMenu(i) => cell_menu(i),
        Action::Save(open_after) => {
            let res = with(|u| u.save_editor());
            match res {
                Some(Ok((l, original))) => {
                    // re-apply a layout that's open right now
                    let was_open = original.as_deref().is_some_and(workspace::is_open) || workspace::is_open(&l.name);
                    if let Some(o) = &original {
                        workspace::close_named(o);
                    }
                    workspace::close_named(&l.name);
                    if open_after || was_open {
                        workspace::open_saved(&l, None);
                    }
                    if open_after {
                        close();
                    } else {
                        with(|u| u.leave_editor());
                        redraw();
                    }
                }
                Some(Err(e)) => {
                    with(|u| {
                        if let Some(ed) = u.editor.as_mut() {
                            ed.error = e;
                        }
                    });
                    redraw();
                }
                None => {}
            }
        }
    }
}

fn popup(owner: HWND, build: impl FnOnce(HMENU)) -> usize {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        build(m);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let c = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, owner, None);
        let _ = DestroyMenu(m);
        c.0 as usize
    }
}

fn card_menu(i: usize) {
    let Some((h, l, is_open)) = with(|u| {
        let l = u.store.layouts.get(i)?.clone();
        let o = u.open.iter().any(|n| n.eq_ignore_ascii_case(&l.name));
        Some((u.hwnd, l, o))
    })
    .flatten() else {
        return;
    };
    let cmd = popup(h, |m| unsafe {
        let _ = AppendMenuW(m, MF_STRING, 1, if is_open { w!("Bring to front") } else { w!("Open") });
        let _ = AppendMenuW(m, MF_STRING, 2, w!("Edit"));
        if is_open {
            let _ = AppendMenuW(m, MF_STRING, 4, w!("Close group"));
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING, 3, w!("Delete"));
    });
    match cmd {
        1 => run(Action::OpenLayout(l)),
        2 => {
            with(|u| u.enter_editor(Some(&l)));
            redraw();
        }
        3 => {
            let q = win::wide(&format!("Delete the layout \u{201C}{}\u{201D}?", l.name));
            let yes = unsafe { MessageBoxW(Some(h), PCWSTR(q.as_ptr()), w!("ZTile"), MB_YESNO | MB_ICONQUESTION) } == IDYES;
            if yes {
                let mut st = Store::load();
                st.remove(&l.name);
                let _ = st.save();
                refresh();
            }
        }
        4 => {
            workspace::close_named(&l.name);
            refresh();
        }
        _ => {}
    }
}

const CM_PICK: usize = 900;
const CM_CLEAR: usize = 901;
const CM_SPLIT_COLS: usize = 902;
const CM_SPLIT_ROWS: usize = 903;
const CM_REMOVE: usize = 904;
const CM_ANY_WINDOW: usize = 905;
const CM_APP: usize = 1000;
const CM_WINDOW: usize = 2000;

fn short_text(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n - 3).collect::<String>() + "..." } else { s.to_string() }
}

/// A window title made into a stable filter: drop unread counters like "(3) ".
fn title_filter(title: &str) -> String {
    let t = title.trim();
    if let Some(rest) = t.strip_prefix('(') {
        if let Some((num, after)) = rest.split_once(')') {
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '+') {
                return after.trim().to_string();
            }
        }
    }
    t.to_string()
}

fn cell_menu(i: usize) {
    let Some((h, cur, multi)) =
        with(|u| u.editor.as_ref().map(|e| (u.hwnd, e.apps.get(i).cloned().flatten(), e.tree.cell_count() > 1))).flatten()
    else {
        return;
    };
    let wins: Vec<apps::OpenWindow> =
        apps::open_windows().into_iter().filter(|w| !w.app.exe_name().eq_ignore_ascii_case("ztile.exe")).collect();
    // one entry per app for "any window of this app"
    let mut app_list: Vec<AppSpec> = Vec::new();
    for w in &wins {
        if !app_list.iter().any(|a| a.same_app(&w.app) && a.aumid == w.app.aumid) {
            app_list.push(w.app.clone());
        }
    }
    let cmd = popup(h, |m| unsafe {
        if let Some(c) = &cur {
            let t = win::wide(&format!("Now: {}", c.label()));
            let _ = AppendMenuW(m, MF_GRAYED, 0, PCWSTR(t.as_ptr()));
            if !c.window.is_empty() {
                let t = win::wide(&format!("Match any {} window", c.display_name()));
                let _ = AppendMenuW(m, MF_STRING, CM_ANY_WINDOW, PCWSTR(t.as_ptr()));
            }
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        }
        let _ = AppendMenuW(m, MF_GRAYED, 0, w!("Open apps:"));
        for (j, a) in app_list.iter().enumerate() {
            let label = win::wide(&short_text(&a.display_name(), 50));
            let _ = AppendMenuW(m, MF_STRING, CM_APP + j, PCWSTR(label.as_ptr()));
        }
        if let Ok(sub) = CreatePopupMenu() {
            for (k, w) in wins.iter().enumerate() {
                let label = win::wide(&format!("{}\t{}", short_text(&w.title, 55), w.app.display_name()));
                let _ = AppendMenuW(sub, MF_STRING, CM_WINDOW + k, PCWSTR(label.as_ptr()));
            }
            let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("A specific window"));
        }
        let _ = AppendMenuW(m, MF_STRING, CM_PICK, w!("Choose a program..."));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        if cur.is_some() {
            let _ = AppendMenuW(m, MF_STRING, CM_CLEAR, w!("Clear cell"));
        }
        let _ = AppendMenuW(m, MF_STRING, CM_SPLIT_COLS, w!("Split side by side"));
        let _ = AppendMenuW(m, MF_STRING, CM_SPLIT_ROWS, w!("Split top and bottom"));
        if multi {
            let _ = AppendMenuW(m, MF_STRING, CM_REMOVE, w!("Remove cell"));
        }
    });
    let picked = match cmd {
        CM_PICK => apps::pick_program(h).map(Some),
        CM_ANY_WINDOW => cur.clone().map(|mut c| {
            c.window.clear();
            Some(c)
        }),
        n if n >= CM_WINDOW => wins.get(n - CM_WINDOW).map(|w| {
            let mut a = w.app.clone();
            a.window = title_filter(&w.title);
            Some(a)
        }),
        n if n >= CM_APP => app_list.get(n - CM_APP).cloned().map(Some),
        _ => None,
    };
    with(|u| {
        let Some(e) = u.editor.as_mut() else { return };
        e.apps.resize(e.tree.cell_count(), None);
        if i >= e.apps.len() {
            return; // the layout changed while the menu was open
        }
        e.error.clear();
        if let Some(spec) = picked {
            e.apps[i] = spec;
        }
        match cmd {
            CM_CLEAR => e.apps[i] = None,
            CM_SPLIT_COLS | CM_SPLIT_ROWS => {
                if e.tree.split_cell(i, if cmd == CM_SPLIT_COLS { Dir::Cols } else { Dir::Rows }) {
                    e.apps.insert(i + 1, None);
                } else {
                    e.error = format!("A layout can have at most {} cells.", layout::MAX_CELLS);
                }
            }
            CM_REMOVE => {
                if e.tree.remove_cell(i) {
                    e.apps.remove(i);
                }
            }
            _ => {}
        }
        e.apps.resize(e.tree.cell_count(), None);
    });
    redraw();
}

fn click(hit: Hit) -> Action {
    with(|u| {
        match hit {
            Hit::New => {
                u.enter_editor(None);
                Action::Redraw
            }
            Hit::Card(i) => u.store.layouts.get(i).cloned().map_or(Action::None, Action::OpenLayout),
            Hit::Back | Hit::Cancel => {
                u.leave_editor();
                Action::Redraw
            }
            Hit::Cell(i) => Action::CellMenu(i),
            Hit::Save => Action::Save(false),
            Hit::SaveOpen => Action::Save(true),
            Hit::Divider(_) => Action::None,
            _ => {
                let Some(e) = u.editor.as_mut() else { return Action::None };
                match hit {
                    Hit::Monitor(i) => e.monitor = i,
                    Hit::Preset(i) => {
                        e.tree = layout::preset(layout::PRESETS[i]).unwrap();
                        e.apps.resize(e.tree.cell_count(), None);
                    }
                    Hit::Borderless => e.borderless = !e.borderless,
                    Hit::KeepChanges => e.keep_changes = !e.keep_changes,
                    Hit::GapMinus => e.gap = (e.gap - 2).max(0),
                    Hit::GapPlus => e.gap = (e.gap + 2).min(40),
                    _ => {}
                }
                Action::Redraw
            }
        }
    })
    .unwrap_or(Action::None)
}

// ---------------------------------------------------------------- window proc

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(h, &mut rc);
                let mem = CreateCompatibleDC(Some(dc));
                let bmp = CreateCompatibleBitmap(dc, rc.right.max(1), rc.bottom.max(1));
                let old = SelectObject(mem, bmp.into());
                if with(|u| u.paint(mem, rc)).is_some() {
                    let _ = BitBlt(dc, 0, 0, rc.right, rc.bottom, Some(mem), 0, 0, SRCCOPY);
                }
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(mem);
                let _ = EndPaint(h, &ps);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_SIZE => {
                let _ = InvalidateRect(Some(h), None, false);
            }
            WM_GETMINMAXINFO => {
                let mmi = &mut *(lp.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(h).max(96) as i32;
                mmi.ptMinTrackSize = POINT { x: 820 * dpi / 96, y: 600 * dpi / 96 };
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT => {
                if let Some((txt, bg, brush)) = with(|u| (u.th.text, u.th.surface, u.edit_brush)) {
                    let dc = HDC(wp.0 as *mut c_void);
                    SetTextColor(dc, COLORREF(txt));
                    SetBkColor(dc, COLORREF(bg));
                    return LRESULT(brush.0 as isize);
                }
            }
            WM_MOUSEMOVE => {
                let p = mouse(lp);
                let changed = with(|u| {
                    if let Some(d) = u.dragging.clone() {
                        if let Some(e) = u.editor.as_mut() {
                            let c = e.canvas;
                            let pos = match d.dir {
                                Dir::Cols => (p.x - c.left) as f32 / (c.right - c.left).max(1) as f32,
                                Dir::Rows => (p.y - c.top) as f32 / (c.bottom - c.top).max(1) as f32,
                            };
                            e.tree.move_divider(&d, pos.clamp(0.0, 1.0));
                            // the divider moved: keep tracking the same one
                            u.dragging = e.tree.dividers().into_iter().find(|x| x.path == d.path && x.k == d.k);
                        }
                        return true;
                    }
                    if !u.tracking {
                        let mut t = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: h,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut t);
                        u.tracking = true;
                    }
                    let hv = u.hit_at(p);
                    let ch = hv != u.hover;
                    u.hover = hv;
                    ch
                })
                .unwrap_or(false);
                if changed {
                    let _ = InvalidateRect(Some(h), None, false);
                }
                return LRESULT(0);
            }
            WM_MOUSELEAVE_ => {
                with(|u| {
                    u.tracking = false;
                    u.hover = None;
                });
                let _ = InvalidateRect(Some(h), None, false);
                return LRESULT(0);
            }
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                let cur = with(|u| match (&u.dragging, u.hover) {
                    (Some(d), _) => Some(if d.dir == Dir::Cols { IDC_SIZEWE } else { IDC_SIZENS }),
                    (None, Some(Hit::Divider(k))) => u.editor.as_ref().and_then(|e| e.dividers.get(k)).map(|d| {
                        if d.dir == Dir::Cols { IDC_SIZEWE } else { IDC_SIZENS }
                    }),
                    (None, Some(hv)) if hv.clickable() => Some(IDC_HAND),
                    _ => None,
                })
                .flatten();
                if let Some(c) = cur {
                    SetCursor(LoadCursorW(None, c).ok());
                    return LRESULT(1);
                }
            }
            WM_LBUTTONDOWN => {
                let p = mouse(lp);
                let grab = with(|u| match u.hit_at(p) {
                    Some(Hit::Divider(k)) => {
                        u.dragging = u.editor.as_ref().and_then(|e| e.dividers.get(k).cloned());
                        u.dragging.is_some()
                    }
                    _ => false,
                })
                .unwrap_or(false);
                if grab {
                    SetCapture(h);
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let p = mouse(lp);
                let was_drag = with(|u| u.dragging.take().is_some()).unwrap_or(false);
                if was_drag {
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(Some(h), None, false);
                    return LRESULT(0);
                }
                if let Some(hit) = with(|u| u.hit_at(p)).flatten() {
                    run(click(hit));
                }
                return LRESULT(0);
            }
            WM_RBUTTONUP => {
                let p = mouse(lp);
                match with(|u| u.hit_at(p)).flatten() {
                    Some(Hit::Card(i)) => run(Action::CardMenu(i)),
                    Some(Hit::Cell(i)) => run(Action::CellMenu(i)),
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                let in_editor = with(|u| u.editor.is_some()).unwrap_or(false);
                if in_editor {
                    with(|u| u.leave_editor());
                    let _ = InvalidateRect(Some(h), None, false);
                } else {
                    close();
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xFFFF) as u32;
                let r = &*(lp.0 as *const RECT);
                with(|u| {
                    u.dpi = dpi;
                    u.fonts = Fonts::new(dpi);
                    if !u.edit.is_invalid() {
                        let nb = u.name_box();
                        let (px, py) = (u.s(10), u.s(8));
                        let _ = SetWindowPos(
                            u.edit,
                            None,
                            nb.left + px,
                            nb.top + py,
                            nb.right - nb.left - 2 * px,
                            nb.bottom - nb.top - 2 * py,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        SendMessageW(u.edit, WM_SETFONT, Some(WPARAM(u.fonts.body.0 as usize)), Some(LPARAM(1)));
                    }
                });
                let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                return LRESULT(0);
            }
            WM_SETTINGCHANGE => {
                // light/dark switch while open
                let dark = system_is_dark();
                let changed = with(|u| {
                    if u.dark == dark {
                        return false;
                    }
                    u.dark = dark;
                    u.th = theme(dark);
                    let _ = DeleteObject(u.edit_brush.into());
                    u.edit_brush = CreateSolidBrush(COLORREF(u.th.surface));
                    true
                })
                .unwrap_or(false);
                if changed {
                    let v = BOOL::from(dark);
                    let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as *const c_void, 4);
                    let _ = InvalidateRect(Some(h), None, true);
                }
            }
            WM_DESTROY => {
                // drop all UI resources: nothing stays in memory while the window is closed
                let icons = with(|u| {
                    (
                        SendMessageW(u.hwnd, WM_GETICON, Some(WPARAM(1)), Some(LPARAM(0))).0,
                        SendMessageW(u.hwnd, WM_GETICON, Some(WPARAM(0)), Some(LPARAM(0))).0,
                    )
                });
                let _ = UI.with(|u| u.try_borrow_mut().map(|mut u| u.take()));
                if let Some((a, b)) = icons {
                    for ic in [a, b] {
                        if ic != 0 {
                            let _ = DestroyIcon(HICON(ic as *mut c_void));
                        }
                    }
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::title_filter;

    #[test]
    fn title_filter_drops_unread_counters() {
        assert_eq!(title_filter("(3) Instagram - Messages"), "Instagram - Messages");
        assert_eq!(title_filter("(99+) Chat"), "Chat");
        assert_eq!(title_filter("Voice Channel | Discord"), "Voice Channel | Discord");
        assert_eq!(title_filter("(beta) Tool"), "(beta) Tool");
    }
}