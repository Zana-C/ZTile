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
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

const USAGE: &str = "\
ZTile — normalde komut gerekmez: ztile.exe'yi çalıştır, tepsiden kullan.
Aşağıdakiler hata ayıklama/otomasyon içindir.

  ztile list
      Monitörleri ve açık uygulama pencerelerini listeler.

  ztile grid [seçenekler] <pencere> <pencere> ...
      Pencereleri bir grup olarak dizer (tek taskbar simgesi).
      <pencere>: başlık/exe parçası (ör. chrome, discord), 0x.. hwnd, ya da + (boş hücre)
      --layout L      hazır dizilim: full 2v 2h 3v 3h 2x2 1+2 1+3 2+1
                      ya da özel: \"cols(60:c, 40:rows(c, c))\"  (c = hücre, sayı = oran)
      --mon N         monitör numarası (list çıktısındaki; varsayılan 0 = birincil)
      --gap N         hücreler arası boşluk, px (varsayılan 0)
      --borderless    klasik başlık çubuklarını kaldır (kendi başlığını çizen
                      Chromium/Electron uygulamalarına dokunmaz)
      --save AD       bu dizilimi ve uygulamalarını AD ismiyle kaydet

  ztile open <AD> [--mon N]
      Kayıtlı dizilimi açar; açık olan uygulamaları bulup yerlerine koyar.
      --mon verilirse kaydedildiği monitör yerine N numaralı monitörde açar.
      Açık olmayanların hücresi + olarak boş kalır. Zaten açıksa öne getirir.

  ztile quit               bütün grupları çözer ve ZTile'ı kapatır
  ztile layouts            hazır ve kayıtlı dizilimleri listeler
  ztile delete <AD>        kayıtlı dizilimi siler
  ztile place <pencere> <x> <y> <genişlik> <yükseklik>

grid/open/quit çalışan ZTile'a iletilir (çalışmıyorsa önce arka planda başlatılır).
";

fn main() {
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
                    unsafe {
                        let _ = AllowSetForegroundWindow(ASFW_ANY);
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
            unsafe {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
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

/// Workspaces live in the tray process: start it if needed, then hand it the command.
fn send_to_daemon(args: &[String]) {
    let mut d = workspace::daemon_window();
    if d.is_none() {
        if args[0] == "quit" {
            println!("Çalışan ZTile yok.");
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        if std::process::Command::new(exe).arg("--tray").spawn().is_err() {
            eprintln!("ZTile başlatılamadı.");
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
        eprintln!("ZTile'a ulaşılamadı.");
        return;
    };
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    if workspace::forward(d, args) {
        println!("ZTile'a iletildi.");
    } else {
        eprintln!("ZTile'a ulaşılamadı.");
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
                eprintln!("  ! pencere bulunamadı: \"{s}\" (hücre boş kalacak)");
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
    println!("Monitörler:");
    for (i, m) in win::monitors().iter().enumerate() {
        println!(
            "  [{i}] {} {}{}  çalışma alanı {}",
            m.device,
            fmt_rect(m.rect),
            if m.primary { " (birincil)" } else { "" },
            fmt_rect(m.work)
        );
    }
    println!("\nPencereler (üstteki önce):");
    for h in win::app_windows() {
        let a = AppSpec::of(h);
        println!(
            "  {:#010x}  {:<22} {:<26} \"{}\"{}{}",
            h.0 as usize,
            a.exe_name(),
            fmt_rect(win::frame_rect(h)),
            short(&a.title, 45),
            if a.aumid.is_empty() { String::new() } else { format!("  [AUMID {}]", short(&a.aumid, 40)) },
            if win::is_fullscreen(h) { "  [tam ekran]" } else { "" }
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
        "{}: hedef {}  sonuç {}  {}",
        win::exe_name(h),
        fmt_rect(target),
        fmt_rect(got),
        if got == target { "OK" } else { "FARKLI (uygulama boyutu dayatıyor olabilir)" }
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
        eprintln!("Bilinmeyen dizilim: {lname}  (ztile layouts ile listele)");
        return;
    };
    let mons = win::monitors();
    let Some(mon) = mons.get(mon_idx).cloned() else {
        eprintln!("Monitör {mon_idx} yok ({} monitör bulundu).", mons.len());
        return;
    };
    let n = tree.cell_count();
    let mut hwnds = resolve(&specs);
    if hwnds.len() > n {
        eprintln!("  ! {} pencere verildi ama dizilim {n} hücreli; fazlası yok sayıldı.", hwnds.len());
    }
    hwnds.resize(n, None);

    if let Some(name) = &save {
        let mut st = Store::load();
        st.put(SavedLayout {
            name: name.clone(),
            monitor: MonitorId::of(&mon),
            tree: tree.clone(),
            gap,
            borderless,
            apps: hwnds.iter().map(|h| h.map(AppSpec::of)).collect(),
        });
        match st.save() {
            Ok(()) => println!("\"{name}\" kaydedildi: {}", Store::path().display()),
            Err(e) => eprintln!("  ! kaydedilemedi: {e}"),
        }
    }
    let _ = workspace::open(workspace::Setup { name: save.unwrap_or_default(), tree, monitor: mon, gap, borderless, hwnds });
}

fn open(a: &[String]) {
    let Some(name) = a.first() else {
        print!("{USAGE}");
        return;
    };
    if workspace::show_named(name) {
        println!("\"{name}\" zaten açık, öne getirildi.");
        return;
    }
    let st = Store::load();
    let Some(l) = st.get(name) else {
        eprintln!("\"{name}\" diye kayıtlı dizilim yok.");
        layouts();
        return;
    };
    let mons = win::monitors();
    let forced = a.iter().position(|s| s == "--mon").and_then(|i| a.get(i + 1)).and_then(|v| v.parse::<usize>().ok());
    let mon = match forced {
        Some(i) => match mons.get(i) {
            Some(m) => m.clone(),
            None => {
                eprintln!("Monitör {i} yok ({} monitör bulundu).", mons.len());
                return;
            }
        },
        None => match l.monitor.find(&mons) {
            Some(m) => m,
            None => {
                eprintln!("Hiç monitör bulunamadı.");
                return;
            }
        },
    };
    workspace::open_saved(l, Some(mon));
}

fn layouts() {
    println!("Hazır dizilimler:");
    for p in layout::PRESETS {
        println!("  {p:<5} {}", layout::preset(p).unwrap().to_spec());
    }
    let st = Store::load();
    println!("\nKayıtlı dizilimler ({}):", Store::path().display());
    if st.layouts.is_empty() {
        println!("  (yok — grid ... --save AD ile kaydet)");
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
        eprintln!("\"{name}\" diye kayıtlı dizilim yok.");
        return;
    }
    match st.save() {
        Ok(()) => println!("\"{name}\" silindi."),
        Err(e) => eprintln!("  ! kaydedilemedi: {e}"),
    }
}
