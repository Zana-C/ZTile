//! Tray icon and its menu, plus "start with Windows".

use crate::store::Store;
use crate::{icon, win, workspace};
use std::cell::Cell;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const ID: u32 = 1;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("ZTile");

const CMD_SHOW: usize = 1;
const CMD_AUTOSTART: usize = 2;
const CMD_QUIT: usize = 3;
const CMD_LAYOUT: usize = 100;

thread_local! {
    static ICON: Cell<isize> = const { Cell::new(0) };
}

fn base() -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: workspace::daemon_hwnd(),
        uID: ID,
        ..Default::default()
    }
}

pub fn add() {
    let mut d = base();
    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    d.uCallbackMessage = workspace::WM_ZT_TRAY;
    let mut ic = ICON.get();
    if ic == 0 {
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
        ic = icon::app_icon(size).0 as isize;
        ICON.set(ic);
    }
    d.hIcon = HICON(ic as *mut _);
    let tip: Vec<u16> = "ZTile".encode_utf16().collect();
    d.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &d);
    }
}

pub fn remove() {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &base());
    }
}

pub fn on_message(lp: LPARAM) {
    match lp.0 as u32 {
        WM_LBUTTONUP => crate::ui::show(),
        WM_RBUTTONUP | WM_CONTEXTMENU => menu(),
        _ => {}
    }
}

fn menu() {
    let st = Store::load();
    let open = workspace::open_names();
    let daemon = workspace::daemon_hwnd();
    let cmd = unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(m, MF_STRING, CMD_SHOW, w!("Open ZTile"));
        let _ = SetMenuDefaultItem(m, CMD_SHOW as u32, 0);
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        if st.layouts.is_empty() {
            let _ = AppendMenuW(m, MF_GRAYED, 0, w!("(no saved layouts)"));
        }
        for (i, l) in st.layouts.iter().enumerate() {
            let is_open = open.iter().any(|n| n.eq_ignore_ascii_case(&l.name));
            let label = win::wide(&l.name);
            let flags = if is_open { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(m, flags, CMD_LAYOUT + i, PCWSTR(label.as_ptr()));
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let auto = if autostart_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(m, auto, CMD_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(m, MF_STRING, CMD_QUIT, w!("Quit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(daemon);
        let c = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, daemon, None);
        let _ = PostMessageW(Some(daemon), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        c.0 as usize
    };
    match cmd {
        CMD_SHOW => crate::ui::show(),
        CMD_AUTOSTART => set_autostart(!autostart_enabled()),
        CMD_QUIT => workspace::quit(),
        n if n >= CMD_LAYOUT => {
            if let Some(l) = st.layouts.get(n - CMD_LAYOUT) {
                workspace::open_saved(l, None);
            }
        }
        _ => {}
    }
}

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let v = win::wide(&format!("\"{exe}\" --tray"));
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(v.as_ptr() as *const _),
                (v.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}
