//! App identity: what we remember about the app in a cell, and how we find its
//! window again later.
//!
//! The exe alone isn't enough: browser PWAs (WhatsApp, Instagram...) all run as
//! the browser's exe and their titles change with every message. Windows gives
//! each of them a distinct AppUserModelID though, so that's the primary key when
//! present. A cell means "any window of this app" unless the user pinned one
//! specific window (matched by a title fragment they chose).

use crate::json::Json;
use crate::win;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_NODEREFERENCELINKS, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Shell::{
    IShellItem, IShellLinkW, SEE_MASK_FLAG_NO_UI, SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SIGDN_NORMALDISPLAY,
    ShellExecuteExW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE;
use windows::core::{Interface, PCWSTR, PWSTR, w};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PVCHF_DEFAULT, PropVariantChangeType, PropVariantClear, PropVariantToStringAlloc,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::core::GUID;

/// What a cell remembers about its app. Deliberately no window titles by default:
/// titles change all the time and often contain private text (chat names...).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AppSpec {
    /// full path of the executable
    pub exe: String,
    /// AppUserModelID of the window ("" if the app doesn't set one)
    pub aumid: String,
    pub class: String,
    /// display name of the app ("Discord", "Instagram"), never a window title
    pub name: String,
    /// "" = any window of this app. Otherwise only windows whose title contains
    /// this text; set only when the user explicitly picks one specific window.
    pub window: String,
}

/// PKEY_AppUserModel_ID
const PKEY_AUMID: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };
/// PKEY_AppUserModel_RelaunchDisplayNameResource: browsers put a PWA's name here
const PKEY_RELAUNCH_NAME: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 4 };

fn window_prop(h: HWND, key: &PROPERTYKEY) -> String {
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(h) else { return String::new() };
        let Ok(mut pv) = store.GetValue(key) else { return String::new() };
        let s = match PropVariantToStringAlloc(&pv) {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            }
            Err(_) => String::new(),
        };
        let _ = PropVariantClear(&mut pv);
        s
    }
}

pub fn aumid(h: HWND) -> String {
    window_prop(h, &PKEY_AUMID)
}

thread_local! {
    /// exe path -> FileDescription (reading version info touches the disk)
    static EXE_NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// "FileDescription" from the exe's version info, e.g. "Discord", "Google Chrome".
fn exe_description(path: &str) -> String {
    if let Some(n) = EXE_NAMES.with(|m| m.borrow().get(path).cloned()) {
        return n;
    }
    let name = unsafe {
        let p = win::wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(p.as_ptr()), None);
        let mut out = String::new();
        if size > 0 && size < 4 * 1024 * 1024 {
            let mut buf = vec![0u8; size as usize];
            if GetFileVersionInfoW(PCWSTR(p.as_ptr()), None, size, buf.as_mut_ptr() as *mut _).is_ok() {
                let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                let mut len = 0u32;
                // first language/codepage pair, then that table's FileDescription
                if VerQueryValueW(buf.as_ptr() as *const _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool()
                    && len >= 4
                {
                    let lang = *(ptr as *const u16);
                    let cp = *(ptr as *const u16).add(1);
                    let q = win::wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
                    if VerQueryValueW(buf.as_ptr() as *const _, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool() && len > 0 {
                        let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                        let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                        out = String::from_utf16_lossy(&s[..n]).trim().to_string();
                    }
                }
            }
        }
        out
    };
    EXE_NAMES.with(|m| m.borrow_mut().insert(path.to_string(), name.clone()));
    name
}

fn exe_stem(path: &str) -> String {
    let file = path.rsplit('\\').next().unwrap_or("");
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_string()
}

thread_local! {
    /// AUMID -> name of its Start menu entry
    static AUMID_NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Name Windows shows for an AppUserModelID in the Start menu ("Instagram",
/// "Calculator"). Works for Store apps and installed browser PWAs.
fn start_menu_name(aumid: &str) -> String {
    if !valid_aumid(aumid) {
        return String::new();
    }
    if let Some(n) = AUMID_NAMES.with(|m| m.borrow().get(aumid).cloned()) {
        return n;
    }
    let name = unsafe {
        let path = win::wide(&format!("shell:AppsFolder\\{aumid}"));
        let item: windows::core::Result<IShellItem> = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None);
        match item {
            Ok(item) => match item.GetDisplayName(SIGDN_NORMALDISPLAY) {
                Ok(p) => {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                }
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        }
    };
    AUMID_NAMES.with(|m| m.borrow_mut().insert(aumid.to_string(), name.clone()));
    name
}

/// Human name of the app a window belongs to (not its title).
fn app_name(h: HWND, exe: &str, aumid: &str) -> String {
    let relaunch = window_prop(h, &PKEY_RELAUNCH_NAME);
    if !relaunch.is_empty() && !relaunch.starts_with('@') {
        return relaunch;
    }
    let menu = start_menu_name(aumid); // e.g. a browser PWA: "Instagram"
    if !menu.is_empty() {
        return menu;
    }
    let d = exe_description(exe);
    if !d.is_empty() { d } else { exe_stem(exe) }
}

/// Start an app without stealing focus where the app allows it.
/// AUMID first (works for Store apps and browser PWAs via their Start menu entry),
/// then the executable.
pub fn launch(spec: &AppSpec) -> bool {
    fn shell_open(target: &str, dir: Option<&str>) -> bool {
        let t = win::wide(target);
        let d = dir.map(win::wide);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_FLAG_NO_UI, // no "Windows can't find..." popups
            lpVerb: w!("open"),
            lpFile: PCWSTR(t.as_ptr()),
            lpDirectory: d.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            nShow: SW_SHOWNOACTIVATE.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info).is_ok() }
    }
    if valid_aumid(&spec.aumid) && shell_open(&format!("shell:AppsFolder\\{}", spec.aumid), None) {
        return true;
    }
    if launchable_exe(&spec.exe) {
        let dir = spec.exe.rsplit_once('\\').map(|(d, _)| d);
        return shell_open(&spec.exe, dir);
    }
    false
}

/// What we're willing to start from a saved layout: a local .exe only
/// (no scripts, documents, shortcuts or network paths), and it must exist.
fn launchable_exe(path: &str) -> bool {
    let b = path.as_bytes();
    let local_abs = b.len() > 3 && b[0].is_ascii_alphabetic() && &b[1..3] == b":\\";
    local_abs && path.to_ascii_lowercase().ends_with(".exe") && std::path::Path::new(path).is_file()
}

/// An AppUserModelID as Windows uses them ("Brave._crx_x", "Microsoft.X_8wekyb3d8bbwe!App",
/// "{GUID}\app.exe"). Rejects anything that could walk elsewhere in the shell namespace.
fn valid_aumid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && !s.contains("..")
        && !s.starts_with('\\')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-!{}\\ ".contains(c))
}

/// Ask the user for a program (.exe or a Start menu shortcut).
pub fn pick_program(owner: HWND) -> Option<AppSpec> {
    let mut buf = [0u16; 1024];
    let filter: Vec<u16> = "Programs and shortcuts\0*.exe;*.lnk\0\0".encode_utf16().collect();
    let start = std::env::var("APPDATA").map(|a| format!("{a}\\Microsoft\\Windows\\Start Menu\\Programs")).unwrap_or_default();
    let start_w = win::wide(&start);
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrInitialDir: PCWSTR(start_w.as_ptr()),
        lpstrTitle: w!("Choose an app"),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NODEREFERENCELINKS | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = String::from_utf16_lossy(&buf[..n]);
    let stem = exe_stem(&path); // a shortcut's own name is the app's name ("Instagram")
    let spec = if path.to_lowercase().ends_with(".lnk") {
        let (exe, aumid) = read_shortcut(&path)?;
        AppSpec { exe, aumid, class: String::new(), name: stem, window: String::new() }
    } else {
        let d = exe_description(&path);
        let name = if d.is_empty() { stem } else { d };
        AppSpec { exe: path, aumid: String::new(), class: String::new(), name, window: String::new() }
    };
    // Only things launch() will actually start (e.g. not a shortcut to a document).
    (launchable_exe(&spec.exe) || valid_aumid(&spec.aumid)).then_some(spec)
}

/// Target exe and AppUserModelID of a .lnk file.
fn read_shortcut(path: &str) -> Option<(String, String)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = link.cast().ok()?;
        let p = win::wide(path);
        pf.Load(PCWSTR(p.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        let _ = link.GetPath(&mut buf, std::ptr::null_mut(), 0);
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let exe = String::from_utf16_lossy(&buf[..n]);
        let mut aumid = String::new();
        if let Ok(store) = link.cast::<IPropertyStore>() {
            if let Ok(mut pv) = store.GetValue(&PKEY_AUMID) {
                if let Ok(s) = PropVariantToStringAlloc(&pv) {
                    aumid = s.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(s.0 as *const _));
                }
                let _ = PropVariantClear(&mut pv);
            }
        }
        (!exe.is_empty() || !aumid.is_empty()).then_some((exe, aumid))
    }
}

/// Give one of *our* windows its own AppUserModelID, so the taskbar shows it as a
/// separate button instead of grouping it with our other windows.
/// Pass "" to clear it (should be done before the window is destroyed).
pub fn set_aumid(h: HWND, id: &str) -> bool {
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(h) else { return false };
        let mut val = PROPVARIANT::default(); // VT_EMPTY clears
        if !id.is_empty() {
            // the shell wants VT_LPWSTR; From<&str> gives VT_BSTR
            let src = PROPVARIANT::from(id);
            if PropVariantChangeType(&mut val, &src, PVCHF_DEFAULT, VT_LPWSTR).is_err() {
                return false;
            }
        }
        store.SetValue(&PKEY_AUMID, &val).is_ok() && store.Commit().is_ok()
    }
}

/// Stable, valid AppUserModelID for a workspace name (ASCII, no spaces, <= 128 chars).
pub fn workspace_aumid(name: &str, fallback_id: u32) -> String {
    if name.is_empty() {
        return format!("ZTile.Group.{fallback_id}");
    }
    let mut s = String::from("ZTile.Group.");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else {
            // keep different names different ("İş" vs "Is")
            s.push_str(&format!("_{:x}", c as u32));
        }
    }
    s.truncate(128);
    s
}

impl AppSpec {
    /// The app of an open window ("any window of this app").
    pub fn of(h: HWND) -> AppSpec {
        let exe = win::exe_path(h);
        let aumid = aumid(h);
        AppSpec { name: app_name(h, &exe, &aumid), aumid, class: win::class(h), exe, window: String::new() }
    }

    pub fn exe_name(&self) -> &str {
        self.exe.rsplit('\\').next().unwrap_or("")
    }

    /// App name for display (falls back to the exe name).
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() { self.name.clone() } else { exe_stem(&self.exe) }
    }

    /// Short human label, including the window filter if the user set one.
    pub fn label(&self) -> String {
        if self.window.is_empty() {
            self.display_name()
        } else {
            let t: String = self.window.chars().take(30).collect();
            format!("{} \u{2013} \u{201C}{t}\u{201D}", self.display_name())
        }
    }

    /// Same program (exe and, if both have one, AppUserModelID)?
    pub fn same_app(&self, o: &AppSpec) -> bool {
        self.exe_name().eq_ignore_ascii_case(o.exe_name())
            && (self.aumid == o.aumid || self.aumid.is_empty() || o.aumid.is_empty())
    }

    pub fn to_json(&self) -> Json {
        let mut v = vec![
            ("exe", Json::str(&self.exe)),
            ("aumid", Json::str(&self.aumid)),
            ("class", Json::str(&self.class)),
            ("name", Json::str(&self.name)),
        ];
        if !self.window.is_empty() {
            v.push(("window", Json::str(&self.window)));
        }
        Json::obj(v)
    }

    pub fn from_json(j: &Json) -> Option<AppSpec> {
        // Older files had a "title" field with whatever the window showed; it's
        // ignored on purpose and disappears on the next save.
        let a = AppSpec {
            exe: j.str_or_empty("exe"),
            aumid: j.str_or_empty("aumid"),
            class: j.str_or_empty("class"),
            name: j.str_or_empty("name"),
            window: j.str_or_empty("window"),
        };
        (!a.exe.is_empty()).then_some(a)
    }

    /// How well an open window (`w`, with its current `title`) matches this spec.
    /// None = can't be it.
    pub fn score(&self, w: &AppSpec, title: &str) -> Option<u32> {
        if !self.exe_name().eq_ignore_ascii_case(w.exe_name()) {
            return None;
        }
        if self.aumid != w.aumid {
            // a PWA must match exactly; a plain app may have gained/lost an id
            if !self.aumid.is_empty() && !w.aumid.is_empty() {
                return None;
            }
        }
        let mut s = 1;
        if !self.window.is_empty() {
            if !title.to_lowercase().contains(&self.window.to_lowercase()) {
                return None; // the user asked for one specific window
            }
            s += 50;
        }
        if !self.aumid.is_empty() && self.aumid == w.aumid {
            s += 100;
        }
        if self.class == w.class {
            s += 10;
        }
        Some(s)
    }
}

/// An open window as a match candidate.
pub struct OpenWindow {
    pub hwnd: HWND,
    pub app: AppSpec,
    pub title: String,
}

pub fn open_windows() -> Vec<OpenWindow> {
    win::app_windows()
        .into_iter()
        .filter(|h| !win::is_fullscreen(*h))
        .map(|h| OpenWindow { hwnd: h, app: AppSpec::of(h), title: win::title(h) })
        .collect()
}

/// Find the best open window for each spec; each window is used at most once.
/// Strongest matches are assigned first so a weak match can't steal a window.
pub fn match_windows(specs: &[Option<AppSpec>]) -> Vec<Option<HWND>> {
    let open = open_windows();
    let mut pairs: Vec<(u32, usize, usize)> = Vec::new(); // (score, spec idx, window idx)
    for (si, spec) in specs.iter().enumerate() {
        let Some(spec) = spec else { continue };
        for (wi, w) in open.iter().enumerate() {
            if let Some(s) = spec.score(&w.app, &w.title) {
                pairs.push((s, si, wi));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = vec![None; specs.len()];
    let mut used = vec![false; open.len()];
    for (_, si, wi) in pairs {
        if out[si].is_none() && !used[wi] {
            out[si] = Some(open[wi].hwnd);
            used[wi] = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(exe: &str, aumid: &str, window: &str) -> AppSpec {
        AppSpec {
            exe: format!("C:\\x\\{exe}"),
            aumid: aumid.into(),
            class: "W".into(),
            name: String::new(),
            window: window.into(),
        }
    }

    #[test]
    fn launch_validation() {
        assert!(valid_aumid("Browser._crx_abcdefghijklmnop"));
        assert!(valid_aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(valid_aumid("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\notepad.exe"));
        assert!(!valid_aumid(""));
        assert!(!valid_aumid("..\\..\\Windows\\System32\\cmd.exe"));
        assert!(!valid_aumid("C:\\evil.exe"));
        assert!(!valid_aumid("http://x/y"));
        assert!(launchable_exe("C:\\Windows\\System32\\notepad.exe"));
        assert!(!launchable_exe("C:\\Windows\\System32\\cmd.bat"));
        assert!(!launchable_exe("\\\\server\\share\\app.exe"));
        assert!(!launchable_exe("relative\\app.exe"));
        assert!(!launchable_exe("C:\\does\\not\\exist.exe"));
    }

    #[test]
    fn workspace_ids_are_valid_and_distinct() {
        assert_eq!(workspace_aumid("Social", 1), "ZTile.Group.social");
        assert_eq!(workspace_aumid("", 7), "ZTile.Group.7");
        assert_ne!(workspace_aumid("İş", 1), workspace_aumid("Is", 1));
        assert!(!workspace_aumid("İş Yeri", 1).contains(' '));
    }

    #[test]
    fn pwa_needs_same_aumid() {
        let wa = spec("brave.exe", "Brave._crx_whatsapp", "");
        assert!(wa.score(&spec("brave.exe", "Brave._crx_insta", ""), "WhatsApp Web").is_none());
        assert!(wa.score(&spec("brave.exe", "Brave._crx_whatsapp", ""), "(3) WhatsApp").unwrap() > 100);
    }

    #[test]
    fn any_window_by_default_specific_window_on_request() {
        let any = spec("Discord.exe", "", "");
        assert!(any.score(&spec("Spotify.exe", "", ""), "Discord").is_none());
        assert!(any.score(&spec("discord.exe", "", ""), "#random | Server - Discord").is_some());
        assert!(any.score(&spec("discord.exe", "", ""), "Friends").is_some());
        let popout = spec("Discord.exe", "", "Voice Channel");
        assert!(popout.score(&spec("discord.exe", "", ""), "Friends - Discord").is_none());
        assert!(popout.score(&spec("discord.exe", "", ""), "voice channel | Discord").is_some());
    }

    #[test]
    fn no_titles_saved_and_old_titles_dropped() {
        let s = spec("Discord.exe", "", "");
        let j = s.to_json();
        assert!(j.get("window").is_none() && j.get("title").is_none());
        let old = Json::parse(r#"{"exe":"C:\\x\\a.exe","aumid":"","class":"W","title":"secret chat"}"#).unwrap();
        let back = AppSpec::from_json(&old).unwrap();
        assert!(!back.to_json().pretty().contains("secret"));
    }
}
