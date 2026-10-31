mod group;
mod icon;
mod layout;
mod win;

use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

const USAGE: &str = "\
ZTile - Faz 0 prototipi

  ztile list
      Monitörleri ve açık uygulama pencerelerini listeler.

  ztile place <pencere> <x> <y> <genişlik> <yükseklik>
      Pencerenin görünen çerçevesini tam olarak bu dikdörtgene oturtur.

  ztile grid [seçenekler] <pencere> <pencere> ...
      Pencereleri bir grup olarak dizer (tek taskbar simgesi).
      <pencere>: başlık/exe parçası (ör. chrome, discord), 0x.. hwnd, ya da + (boş hücre)
      --layout full|2v|2h|3v|3h|2x2|1+2|1+3|2+1   (varsayılan: pencere sayısına göre)
      --mon N         monitör numarası (list çıktısındaki; varsayılan 0 = birincil)
      --gap N         hücreler arası boşluk, px (varsayılan 0)
      --borderless    klasik başlık çubuklarını kaldır (kendi başlığını çizen
                      Chromium/Electron uygulamalarına dokunmaz)

Örnek:  ztile grid --layout 2x2 chrome spotify discord +
";

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("place") => place(&args[1..]),
        Some("grid") => grid(&args[1..]),
        _ => print!("{USAGE}"),
    }
}

fn fmt_rect(r: RECT) -> String {
    format!("{}x{} @ ({},{})", r.right - r.left, r.bottom - r.top, r.left, r.top)
}

fn list() {
    println!("Monitörler:");
    for (i, m) in win::monitors().iter().enumerate() {
        println!(
            "  [{i}] {}{}  çalışma alanı {}",
            fmt_rect(m.rect),
            if m.primary { " (birincil)" } else { "" },
            fmt_rect(m.work)
        );
    }
    println!("\nPencereler (üstteki önce):");
    for h in win::app_windows() {
        let (wr, fr) = (win::window_rect(h), win::frame_rect(h));
        let mut t = win::title(h);
        if t.chars().count() > 50 {
            t = t.chars().take(47).collect::<String>() + "...";
        }
        println!(
            "  {:#010x}  {:<22} {:<28} \"{}\"  görünmez kenar L{} T{} R{} B{}{}",
            h.0 as usize,
            win::exe_name(h),
            win::class(h),
            t,
            fr.left - wr.left,
            fr.top - wr.top,
            wr.right - fr.right,
            wr.bottom - fr.bottom,
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
    let Some(h) = group::resolve(&a[..1]).into_iter().next().flatten() else { return };
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
    let mut opt = group::Options { layout: None, monitor: 0, gap: 0, borderless: false, specs: Vec::new() };
    let mut it = a.iter();
    while let Some(s) = it.next() {
        match s.as_str() {
            "--layout" => opt.layout = it.next().cloned(),
            "--mon" => opt.monitor = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--gap" => opt.gap = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--borderless" => opt.borderless = true,            _ => opt.specs.push(s.clone()),
        }
    }
    if opt.specs.is_empty() && opt.layout.is_none() {
        print!("{USAGE}");
        return;
    }
    group::run(opt);
}
