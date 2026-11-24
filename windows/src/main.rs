// A tray app: no console window. CLI subcommands attach to the parent console.
#![windows_subsystem = "windows"]

mod apps;
mod icon;
mod json;
mod layout;
mod store;
mod tray;
mod ui;
mod win;
mod workspace;

use apps::AppSpec;
use store::{MonitorId, SavedLayout, Store};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const USAGE: &str = "\
ZTile normally needs no commands: run ztile.exe and use it from the tray.
The commands below are for scripting and troubleshooting.

  ztile list
      Lists displays and open app windows.

  ztile grid [options] <window> <window> ...
      Tiles windows as one group (one taskbar button).
      <window>: part of a title or exe name (e.g. chrome, discord), a 0x.. hwnd, or + (empty cell)
      --layout L      preset: full 2v 2h 3v 3h 2x2 1+2 1+3 2+1
                      or custom: \"cols(60:c, 40:rows(c, c))\"  (c = cell, number = share)
      --mon N         display number from `list` (default 0 = primary)
      --gap N         gap between cells in px (default 0)
      --borderless    hide classic title bars (apps that draw their own, like
                      Chromium/Electron apps, are left alone)
      --save NAME     save this layout and its apps as NAME

  ztile open <NAME> [--mon N]
      Opens a saved layout: running apps are moved into place, missing ones are
      started. --mon opens it on display N instead of its saved display.
      Already open: brings it to the front.

  ztile quit               dissolves all groups and exits ZTile
  ztile layouts            lists presets and saved layouts
  ztile delete <NAME>      deletes a saved layout
  ztile place <window> <x> <y> <width> <height>

grid/open/quit are sent to the running ZTile (started in the background if needed).
";

/// Runtime hardening for DLLs loaded later (e.g. by the file dialog):
/// no DLLs from network shares or low-integrity locations, System32 first,
/// and no legacy injection points (AppInit DLLs, global hooks of other apps).
fn harden() {
    use windows::Win32::System::Threading::{
        ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy, SetProcessMitigationPolicy,
    };
    // PROCESS_MITIGATION_IMAGE_LOAD_POLICY: NoRemoteImages | NoLowMandatoryLabelImages | PreferSystem32Images
    let image_load: u32 = 0b111;
    // PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY: DisableExtensionPoints
    let ext_points: u32 = 0b1;
    unsafe {
        let _ = SetProcessMitigationPolicy(ProcessImageLoadPolicy, &image_load as *const u32 as *const _, 4);
        let _ = SetProcessMitigationPolicy(ProcessExtensionPointDisablePolicy, &ext_points as *const u32 as *const _, 4);
    }
}

fn main() {
    harden();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        // Normal start (double-click, Start menu) or autostart (--tray).
        None | Some("--tray") => {
            let tray_only = !args.is_empty();
            if let Some(d) = workspace::daemon_window() {
                // Already running: just show its window.
                if !tray_only {
                    // let only ZTile itself (not any process) bring its window up
                    unsafe {
                        let _ = AllowSetForegroundWindow(win::pid(d));
                    }
                    workspace::forward(d, &["show".to_string()]);
                }
                return;
            }
            if !workspace::init(execute) {
                return;
            }
            tray::add();
            if !tray_only {
                ui::show();
            }
            workspace::run_loop();
        }
        Some(cmd) => {
            attach_console();
            match cmd {
                "list" => list(),
                "place" => place(rest),
                "layouts" => layouts(),
                "delete" => delete(rest),
                "grid" | "open" | "quit" | "show" => send_to_daemon(&args),
                _ => print!("{USAGE}"),
            }
        }
    }
}

/// We're a GUI-subsystem program: to print in the terminal we were started from,
/// attach to its console and point stdout/stderr at it (unless they're redirected).
fn attach_console() {
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let missing = GetStdHandle(id).map(|h| h.is_invalid() || h.0.is_null()).unwrap_or(true);
            if missing {
                if let Ok(con) = CreateFileW(
                    windows::core::w!("CONOUT$"),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) {
                    let _ = SetStdHandle(id, con);
                }
            }
        }
    }
}

/// Workspaces live in the tray process: start it if needed, then hand it the command.
fn send_to_daemon(args: &[String]) {
    let mut d = workspace::daemon_window();
    if d.is_none() {
        if args[0] == "quit" {
            println!("ZTile isn't running.");
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        if std::process::Command::new(exe).arg("--tray").spawn().is_err() {
            eprintln!("Couldn't start ZTile.");
            return;
        }
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            d = workspace::daemon_window();
            if d.is_some() {
                break;
            }
        }
    }
    let Some(d) = d else {
        eprintln!("Couldn't reach ZTile.");
        return;
    };
    unsafe {
        let _ = AllowSetForegroundWindow(win::pid(d));
    }
    if workspace::forward(d, args) {
        println!("Sent to ZTile.");
    } else {
        eprintln!("Couldn't reach ZTile.");
    }
}

/// Commands that act on workspaces; runs inside the tray process.
fn execute(args: &[String]) {
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().map(String::as_str) {
        Some("grid") => grid(rest),
        Some("open") => open(rest),
        Some("show") => ui::show(),
        Some("quit") => workspace::quit(),
        _ => {}
    }
}

/// Window specs from the command line: title/exe fragment, 0x.. hwnd, or + (empty).
fn resolve(specs: &[String]) -> Vec<Option<HWND>> {
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
            if let Some(h) = found {
                used.push(h);
            } else {
                eprintln!("  ! no window matches \"{s}\" (cell stays empty)");
            }
            found
        })
        .collect()
}

fn fmt_rect(r: RECT) -> String {
    format!("{}x{} @ ({},{})", r.right - r.left, r.bottom - r.top, r.left, r.top)
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n { s.chars().take(n - 3).collect::<String>() + "..." } else { s.to_string() }
}

fn list() {
    println!("Displays:");
    for (i, m) in win::monitors().iter().enumerate() {
        println!(
            "  [{i}] {} {}{}  work area {}",
            m.device,
            fmt_rect(m.rect),
            if m.primary { " (primary)" } else { "" },
            fmt_rect(m.work)
        );
    }
    println!("\nWindows (topmost first):");
    for h in win::app_windows() {
        let a = AppSpec::of(h);
        println!(
            "  {:#010x}  {:<22} {:<26} {:<18} \"{}\"{}{}",
            h.0 as usize,
            a.exe_name(),
            fmt_rect(win::frame_rect(h)),
            short(&a.display_name(), 18),
            short(&win::title(h), 40),
            if a.aumid.is_empty() { String::new() } else { format!("  [AUMID {}]", short(&a.aumid, 40)) },
            if win::is_fullscreen(h) { "  [fullscreen]" } else { "" }
        );
    }
}

fn place(a: &[String]) {
    let nums: Vec<i32> = a.iter().skip(1).filter_map(|s| s.parse().ok()).collect();
    if a.is_empty() || nums.len() != 4 {
        print!("{USAGE}");
        return;
    }
    let Some(h) = resolve(&a[..1]).into_iter().next().flatten() else { return };
    let target = RECT { left: nums[0], top: nums[1], right: nums[0] + nums[2], bottom: nums[1] + nums[3] };
    win::place(h, target);
    let got = win::frame_rect(h);
    println!(
        "{}: wanted {}  got {}  {}",
        win::exe_name(h),
        fmt_rect(target),
        fmt_rect(got),
        if got == target { "OK" } else { "DIFFERENT (the app may enforce its own size)" }
    );
}

fn grid(a: &[String]) {
    let (mut layout_arg, mut mon_idx, mut gap, mut borderless) = (None, 0usize, 0, false);
    let (mut save, mut specs) = (None, Vec::new());
    let mut it = a.iter();
    while let Some(s) = it.next() {
        match s.as_str() {
            "--layout" => layout_arg = it.next().cloned(),
            "--mon" => mon_idx = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--gap" => gap = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--borderless" => borderless = true,
            "--save" => save = it.next().cloned(),
            _ => specs.push(s.clone()),
        }
    }
    if specs.is_empty() && layout_arg.is_none() {
        print!("{USAGE}");
        return;
    }
    let lname = layout_arg.unwrap_or_else(|| layout::default_for(specs.len()).to_string());
    let Some(tree) = layout::from_name_or_spec(&lname) else {
        eprintln!("Unknown layout: {lname}  (see `ztile layouts`)");
        return;
    };
    let mons = win::monitors();
    let Some(mon) = mons.get(mon_idx).cloned() else {
        eprintln!("There is no display {mon_idx} ({} found).", mons.len());
        return;
    };
    let n = tree.cell_count();
    let mut hwnds = resolve(&specs);
    if hwnds.len() > n {
        eprintln!("  ! {} windows given but the layout has {n} cells; the rest are ignored.", hwnds.len());
    }
    hwnds.resize(n, None);

    let specs: Vec<Option<AppSpec>> = hwnds.iter().map(|h| h.map(AppSpec::of)).collect();
    if let Some(name) = &save {
        let mut st = Store::load();
        st.put(SavedLayout {
            name: name.clone(),
            monitor: MonitorId::of(&mon),
            tree: tree.clone(),
            gap,
            borderless,
            apps: specs.clone(),
            keep_changes: false,
        });
        match st.save() {
            Ok(()) => println!("Saved \"{name}\" to {}", Store::path().display()),
            Err(e) => eprintln!("  ! couldn't save: {e}"),
        }
    }
    let _ = workspace::open(workspace::Setup {
        name: save.unwrap_or_default(),
        tree,
        monitor: mon,
        gap,
        borderless,
        hwnds,
        specs,
        keep_changes: false,
    });
}

fn open(a: &[String]) {
    let Some(name) = a.first() else {
        print!("{USAGE}");
        return;
    };
    if workspace::show_named(name) {
        println!("\"{name}\" is already open; brought it to the front.");
        return;
    }
    let st = Store::load();
    let Some(l) = st.get(name) else {
        eprintln!("No saved layout named \"{name}\".");
        layouts();
        return;
    };
    let mons = win::monitors();
    let forced = a.iter().position(|s| s == "--mon").and_then(|i| a.get(i + 1)).and_then(|v| v.parse::<usize>().ok());
    let mon = match forced {
        Some(i) => match mons.get(i) {
            Some(m) => m.clone(),
            None => {
                eprintln!("There is no display {i} ({} found).", mons.len());
                return;
            }
        },
        None => match l.monitor.find(&mons) {
            Some(m) => m,
            None => {
                eprintln!("No displays found.");
                return;
            }
        },
    };
    workspace::open_saved(l, Some(mon));
}

fn layouts() {
    println!("Presets:");
    for p in layout::PRESETS {
        println!("  {p:<5} {}", layout::preset(p).unwrap().to_spec());
    }
    let st = Store::load();
    println!("\nSaved layouts ({}):", Store::path().display());
    if st.layouts.is_empty() {
        println!("  (none yet: create one in the ZTile window)");
    }
    for l in &st.layouts {
        let apps: Vec<String> = l.apps.iter().map(|a| a.as_ref().map_or("+".to_string(), AppSpec::label)).collect();
        println!("  {:<12} {}  [{}]", l.name, l.tree.to_spec(), apps.join(" | "));
    }
}

fn delete(a: &[String]) {
    let Some(name) = a.first() else {
        print!("{USAGE}");
        return;
    };
    let mut st = Store::load();
    if !st.remove(name) {
        eprintln!("No saved layout named \"{name}\".");
        return;
    }
    match st.save() {
        Ok(()) => println!("Deleted \"{name}\"."),
        Err(e) => eprintln!("  ! couldn't save: {e}"),
    }
}
