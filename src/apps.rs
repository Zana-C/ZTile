//! App identity: what we remember about the app in a cell, and how we find its
//! window again later.
//!
//! The exe alone isn't enough: Brave/Chrome PWAs (WhatsApp, Instagram...) all run
//! as brave.exe and their titles change with every message. Windows gives each
//! of them a distinct AppUserModelID though, so that's the primary key when present.

use crate::json::Json;
use crate::win;
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_NODEREFERENCELINKS, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Shell::{IShellLinkW, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE;
use windows::core::{Interface, PCWSTR, PWSTR, w};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PVCHF_DEFAULT, PropVariantChangeType, PropVariantClear, PropVariantToStringAlloc,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::core::GUID;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct AppSpec {
    /// full path of the executable
    pub exe: String,
    /// AppUserModelID of the window ("" if the app doesn't set one)
    pub aumid: String,
    pub class: String,
    /// title when saved; only a weak hint, titles change
    pub title: String,
}

/// PKEY_AppUserModel_ID
const PKEY_AUMID: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };

pub fn aumid(h: HWND) -> String {
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(h) else { return String::new() };
        let Ok(mut pv) = store.GetValue(&PKEY_AUMID) else { return String::new() };
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
    if !spec.aumid.is_empty() && shell_open(&format!("shell:AppsFolder\\{}", spec.aumid), None) {
        return true;
    }
    if std::path::Path::new(&spec.exe).is_file() {
        let dir = spec.exe.rsplit_once('\\').map(|(d, _)| d);
        return shell_open(&spec.exe, dir);
    }
    false
}

/// Ask the user for a program (.exe or a Start menu shortcut).
pub fn pick_program(owner: HWND) -> Option<AppSpec> {
    let mut buf = [0u16; 1024];
    let filter: Vec<u16> = "Programlar ve kısayollar\0*.exe;*.lnk\0Tüm dosyalar\0*.*\0\0".encode_utf16().collect();
    let start = std::env::var("APPDATA").map(|a| format!("{a}\\Microsoft\\Windows\\Start Menu\\Programs")).unwrap_or_default();
    let start_w = win::wide(&start);
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrInitialDir: PCWSTR(start_w.as_ptr()),
        lpstrTitle: w!("Uygulama seç"),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NODEREFERENCELINKS | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = String::from_utf16_lossy(&buf[..n]);
    let stem = path.rsplit('\\').next().unwrap_or("").rsplit_once('.').map_or("", |(s, _)| s).to_string();
    if path.to_lowercase().ends_with(".lnk") {
        let (exe, aumid) = read_shortcut(&path)?;
        return Some(AppSpec { exe, aumid, class: String::new(), title: stem });
    }
    Some(AppSpec { exe: path, aumid: String::new(), class: String::new(), title: stem })
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
    pub fn of(h: HWND) -> AppSpec {
        AppSpec { exe: win::exe_path(h), aumid: aumid(h), class: win::class(h), title: win::title(h) }
    }

    pub fn exe_name(&self) -> &str {
        self.exe.rsplit('\\').next().unwrap_or("")
    }

    /// Short human label: title for PWAs (exe is just the browser), else exe name.
    pub fn label(&self) -> String {
        if !self.aumid.is_empty() && !self.title.is_empty() {
            let t: String = self.title.chars().take(30).collect();
            format!("{} ({t})", self.exe_name())
        } else {
            self.exe_name().to_string()
        }
    }

    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("exe", Json::str(&self.exe)),
            ("aumid", Json::str(&self.aumid)),
            ("class", Json::str(&self.class)),
            ("title", Json::str(&self.title)),
        ])
    }

    pub fn from_json(j: &Json) -> Option<AppSpec> {
        let a = AppSpec {
            exe: j.str_or_empty("exe"),
            aumid: j.str_or_empty("aumid"),
            class: j.str_or_empty("class"),
            title: j.str_or_empty("title"),
        };
        (!a.exe.is_empty()).then_some(a)
    }

    /// How well an open window matches this spec. None = can't be it.
    pub fn score(&self, w: &AppSpec) -> Option<u32> {
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
        if !self.aumid.is_empty() && self.aumid == w.aumid {
            s += 100;
        }
        if self.class == w.class {
            s += 10;
        }
        s += word_overlap(&self.title, &w.title);
        Some(s)
    }
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_lowercase).collect()
}

fn word_overlap(a: &str, b: &str) -> u32 {
    let wb = words(b);
    words(a).iter().filter(|w| wb.contains(w)).count() as u32
}

/// Find the best open window for each spec; each window is used at most once.
/// Strongest matches are assigned first so a weak match can't steal a window.
pub fn match_windows(specs: &[Option<AppSpec>]) -> Vec<Option<HWND>> {
    let open: Vec<(HWND, AppSpec)> =
        win::app_windows().into_iter().filter(|h| !win::is_fullscreen(*h)).map(|h| (h, AppSpec::of(h))).collect();
    let mut pairs: Vec<(u32, usize, usize)> = Vec::new(); // (score, spec idx, window idx)
    for (si, spec) in specs.iter().enumerate() {
        let Some(spec) = spec else { continue };
        for (wi, (_, w)) in open.iter().enumerate() {
            if let Some(s) = spec.score(w) {
                pairs.push((s, si, wi));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = vec![None; specs.len()];
    let mut used = vec![false; open.len()];
    for (_, si, wi) in pairs {
        if out[si].is_none() && !used[wi] {
            out[si] = Some(open[wi].0);
            used[wi] = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(exe: &str, aumid: &str, title: &str) -> AppSpec {
        AppSpec { exe: format!("C:\\x\\{exe}"), aumid: aumid.into(), class: "W".into(), title: title.into() }
    }

    #[test]
    fn workspace_ids_are_valid_and_distinct() {
        assert_eq!(workspace_aumid("Sosyal", 1), "ZTile.Group.sosyal");
        assert_eq!(workspace_aumid("", 7), "ZTile.Group.7");
        assert_ne!(workspace_aumid("İş", 1), workspace_aumid("Is", 1));
        assert!(!workspace_aumid("İş Yeri", 1).contains(' '));
    }

    #[test]
    fn pwa_needs_same_aumid() {
        let wa = spec("brave.exe", "Brave._crx_whatsapp", "WhatsApp Web");
        assert!(wa.score(&spec("brave.exe", "Brave._crx_insta", "WhatsApp Web")).is_none());
        assert!(wa.score(&spec("brave.exe", "Brave._crx_whatsapp", "(3) WhatsApp")).unwrap() > 100);
    }

    #[test]
    fn exe_must_match_and_title_is_a_hint() {
        let d = spec("Discord.exe", "", "#genel | Server - Discord");
        assert!(d.score(&spec("Spotify.exe", "", "Discord")).is_none());
        let a = d.score(&spec("discord.exe", "", "#random | Server - Discord")).unwrap();
        let b = d.score(&spec("discord.exe", "", "Friends")).unwrap();
        assert!(a > b);
    }
}
