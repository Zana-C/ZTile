//! Low-level window helpers: enumeration, filtering, placement.

use std::ffi::c_void;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PWSTR};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn class(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

pub fn exe_path(h: HWND) -> String {
    unsafe {
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid(h)) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(proc);
        if ok { String::from_utf16_lossy(&buf[..len as usize]) } else { String::new() }
    }
}

pub fn exe_name(h: HWND) -> String {
    let p = exe_path(h);
    p.rsplit('\\').next().unwrap_or("").to_string()
}

fn style(h: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(h, GWL_STYLE) as u32 }
}

fn ex_style(h: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 }
}

pub fn is_cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe {
        DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut _ as *mut c_void, 4).is_ok() && v != 0
    }
}

pub fn window_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

/// Visible frame (without the invisible resize borders of Win10/11).
pub fn frame_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .is_err()
        {
            return window_rect(h);
        }
    }
    r
}

pub fn is_minimized(h: HWND) -> bool {
    unsafe { IsIconic(h).as_bool() }
}

pub fn is_maximized(h: HWND) -> bool {
    unsafe { IsZoomed(h).as_bool() }
}

pub fn is_topmost(h: HWND) -> bool {
    ex_style(h) & WS_EX_TOPMOST.0 != 0
}

pub fn is_alive(h: HWND) -> bool {
    unsafe { IsWindow(Some(h)).as_bool() }
}

/// A window that covers its whole monitor with no caption (games, video players).
pub fn is_fullscreen(h: HWND) -> bool {
    if style(h) & WS_CAPTION.0 == WS_CAPTION.0 {
        return false;
    }
    let m = monitor_of(h);
    let r = window_rect(h);
    r.left <= m.rect.left && r.top <= m.rect.top && r.right >= m.rect.right && r.bottom >= m.rect.bottom
}

const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "ZTileGroup",
    "ZTilePlus",
    "ZTileMain",
    "ZTileDaemon",
    "ZTileOverlay",
];

/// "Real" app window: what you'd expect to see on the taskbar / Alt+Tab.
pub fn is_app_window(h: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return false;
        }
        if GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false) {
            return false;
        }
    }
    let ex = ex_style(h);
    if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
        return false;
    }
    if ex & WS_EX_NOACTIVATE.0 != 0 {
        return false;
    }
    if is_cloaked(h) || title(h).is_empty() {
        return false;
    }
    let c = class(h);
    !SHELL_CLASSES.contains(&c.as_str())
}

pub fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let v = unsafe { &mut *(l.0 as *mut Vec<HWND>) };
        if is_app_window(h) {
            v.push(h);
        }
        true.into()
    }
    let mut v: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub rect: RECT,
    pub work: RECT,
    pub primary: bool,
    /// GDI device name, e.g. \\.\DISPLAY1
    pub device: String,
}

fn monitor_info(m: HMONITOR) -> Monitor {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    unsafe {
        let _ = GetMonitorInfoW(m, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO);
    }
    let dev = &mi.szDevice;
    let n = dev.iter().position(|&c| c == 0).unwrap_or(dev.len());
    let i = mi.monitorInfo;
    Monitor {
        rect: i.rcMonitor,
        work: i.rcWork,
        primary: i.dwFlags & MONITORINFOF_PRIMARY != 0,
        device: String::from_utf16_lossy(&dev[..n]),
    }
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, l: LPARAM) -> BOOL {
        let v = unsafe { &mut *(l.0 as *mut Vec<Monitor>) };
        v.push(monitor_info(m));
        true.into()
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    // primary first, then left-to-right
    v.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    v
}

pub fn monitor_of(h: HWND) -> Monitor {
    monitor_info(unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) })
}

/// Move/resize so that the *visible* frame exactly matches `target`.
pub fn place(h: HWND, target: RECT) {
    unsafe {
        if is_minimized(h) || is_maximized(h) {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            if is_maximized(h) {
                let _ = ShowWindow(h, SW_RESTORE);
            }
        }
        // Twice: crossing monitors with different DPI makes the app rescale itself
        // after the first move, which changes the border offsets.
        for _ in 0..2 {
            let wr = window_rect(h);
            let fr = frame_rect(h);
            let (l, t) = (fr.left - wr.left, fr.top - wr.top);
            let (r, b) = (wr.right - fr.right, wr.bottom - fr.bottom);
            let _ = SetWindowPos(
                h,
                None,
                target.left - l,
                target.top - t,
                (target.right - target.left) + l + r,
                (target.bottom - target.top) + t + b,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
            if frame_rect(h) == target {
                break;
            }
        }
    }
}

/// Remove caption + sizing border. Returns the original style for restoring.
pub fn make_borderless(h: HWND) -> u32 {
    let old = style(h);
    let new = old & !(WS_CAPTION.0 | WS_THICKFRAME.0);
    unsafe {
        SetWindowLongPtrW(h, GWL_STYLE, new as isize);
        let _ = SetWindowPos(
            h,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    old
}

pub fn restore_style(h: HWND, old: u32) {
    unsafe {
        SetWindowLongPtrW(h, GWL_STYLE, old as isize);
        let _ = SetWindowPos(
            h,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Put a window on top of the normal z-order without activating it.
/// Topmost-then-not-topmost works even when we're not the foreground process.
pub fn raise_no_activate(h: HWND) {
    if is_topmost(h) {
        return; // user's own always-on-top window: leave it alone
    }
    let f = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    unsafe {
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, f);
        let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, f);
    }
}

fn dwm_set_u32(h: HWND, attr: DWMWINDOWATTRIBUTE, v: u32) {
    unsafe {
        let _ = DwmSetWindowAttribute(h, attr, &v as *const u32 as *const c_void, 4);
    }
}

/// Win11 rounds corners of windows that aren't snapped; tiles look gappy with that.
pub fn set_square_corners(h: HWND, square: bool) {
    // DWMWCP_DEFAULT = 0, DWMWCP_DONOTROUND = 1
    dwm_set_u32(h, DWMWA_WINDOW_CORNER_PREFERENCE, if square { 1 } else { 0 });
    refresh_frame(h);
}

/// Make DWM re-render the non-client frame now (attribute changes otherwise
/// only show up on the next activation/resize).
pub fn refresh_frame(h: HWND) {
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
        );
    }
}

/// Hide (or restore) the 1px DWM accent border.
pub fn set_border_hidden(h: HWND, hidden: bool) {
    // DWMWA_COLOR_NONE = 0xFFFFFFFE, DWMWA_COLOR_DEFAULT = 0xFFFFFFFF
    dwm_set_u32(h, DWMWA_BORDER_COLOR, if hidden { 0xFFFF_FFFE } else { 0xFFFF_FFFF });
}

/// Height of the system-drawn caption. ~0 for apps that draw their own title bar
/// (Chromium/Electron: Brave, Discord, Spotify, VS Code...).
pub fn native_caption_height(h: HWND) -> i32 {
    let mut p = POINT::default();
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut p);
    }
    p.y - frame_rect(h).top
}

pub fn contains(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}
