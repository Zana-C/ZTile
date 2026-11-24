//! ZTile for KDE Plasma: entry point, single instance, command line, event loop.

mod apps;
mod bus;
mod daemon;
mod files;
mod icon;
mod store;
mod tray;
mod wl;

use bus::{Bus, IFACE, OBJECT, service};
use daemon::Daemon;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Instant;
use ztile_core::layout;

const USAGE: &str = "\
ZTile normally needs no commands: run it and use it from the tray.

  ztile                 open the ZTile window (starts ZTile if needed)
  ztile --tray          start in the tray only
  ztile open <NAME>     open a saved layout (bring it up if it's open)
  ztile close <NAME>    dissolve an open group
  ztile layouts         list presets and saved layouts
  ztile list            list displays and open groups
  ztile quit            dissolve all groups and exit
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            return;
        }
        Some("layouts") => {
            layouts();
            return;
        }
        _ => {}
    }
    let tray_only = args.first().map(String::as_str) == Some("--tray");
    let cmd: Vec<String> = if args.is_empty() || tray_only { vec!["show".into()] } else { args.clone() };

    let bus = match Bus::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ztile: {e}");
            std::process::exit(1);
        }
    };
    // Already running: hand it the command.
    if bus.name_owner(service()).is_some() {
        if tray_only {
            return;
        }
        let payload = ztile_core::json::Json::Arr(cmd.iter().map(|a| ztile_core::json::Json::str(a)).collect()).compact();
        match bus.call(service(), OBJECT, IFACE, "command", |m| m.append1(payload)) {
            Ok(r) => {
                let text: String = r.get1().unwrap_or_default();
                if !text.is_empty() {
                    println!("{text}");
                }
            }
            Err(e) => eprintln!("ztile: couldn't reach the running ZTile ({e})"),
        }
        return;
    }
    if matches!(cmd[0].as_str(), "quit" | "close" | "list") {
        println!("ZTile isn't running.");
        return;
    }
    if !matches!(cmd[0].as_str(), "show" | "open") {
        print!("{USAGE}");
        return;
    }
    if !bus.request_name(service()) {
        if !tray_only {
            eprintln!("ztile: another copy is starting right now");
        }
        return;
    }
    let wl = match wl::Wl::connect() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("ztile: {e}. ZTile needs a KDE Plasma (Wayland) session.");
            std::process::exit(1);
        }
    };
    let Some(script) = find_data("kwin/contents/code/main.js") else {
        eprintln!("ztile: can't find its KWin script (kwin/contents/code/main.js next to the program)");
        std::process::exit(1);
    };
    let ui_dir = find_data("ui/ztile_ui/__main__.py").and_then(|p| p.parent()?.parent().map(Path::to_path_buf));
    let mut d = Daemon::new(bus, wl, script, ui_dir);
    if let Err(e) = d.start() {
        eprintln!("ztile: {e}");
        std::process::exit(1);
    }
    match cmd[0].as_str() {
        "open" => {
            if let Some(l) = store::Store::load().get(cmd.get(1).map(String::as_str).unwrap_or("")).cloned() {
                d.open_saved(&l);
            } else {
                eprintln!("ztile: no saved layout named \"{}\"", cmd.get(1).map(String::as_str).unwrap_or(""));
            }
        }
        _ if !tray_only => d.show_window(),
        _ => {}
    }
    run(&mut d);
}

/// Data files sit next to the program: <exe dir>/ (portable build in linux/),
/// ../share/ztile/ (installed to ~/.local/bin), or the source tree (cargo run).
fn find_data(rel: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.to_path_buf(),
        dir.join("../share/ztile"),
        dir.join("../.."), // linux/target/release/ztile -> linux/
    ];
    candidates.iter().map(|c| c.join(rel)).find(|p| p.is_file()).and_then(|p| p.canonicalize().ok())
}

fn layouts() {
    println!("Presets:");
    for p in layout::PRESETS {
        println!("  {p:<5} {}", layout::preset(p).unwrap().to_spec());
    }
    let st = store::Store::load();
    println!("\nSaved layouts ({}):", store::Store::path().display());
    if st.layouts.is_empty() {
        println!("  (none yet: create one in the ZTile window)");
    }
    for l in &st.layouts {
        let apps: Vec<String> = l
            .apps
            .iter()
            .map(|a| a.as_ref().map_or("+".to_string(), |a| if a.name.is_empty() { a.app.clone() } else { a.name.clone() }))
            .collect();
        println!("  {:<12} {}  [{}]", l.name, l.tree.to_spec(), apps.join(" | "));
    }
}

// ---------------------------------------------------------------- event loop

static SIG_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIG_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        unsafe {
            libc::write(fd, b"x".as_ptr() as *const _, 1);
        }
    }
}

/// SIGTERM/SIGINT/SIGHUP (logout, `kill`): give the windows back before exiting.
fn signal_fd() -> i32 {
    let mut fds = [0i32; 2];
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) != 0 {
            return -1;
        }
        SIG_PIPE.store(fds[1], Ordering::Relaxed);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    fds[0]
}

fn run(d: &mut Daemon) {
    let sig = signal_fd();
    let mut wl_dead = false;
    loop {
        // Wayland: handle what's queued, send what we wrote, then wait on all fds.
        if !wl_dead && d.wl.queue.dispatch_pending(&mut d.wl.state).is_err() {
            eprintln!("ztile: lost the Wayland connection");
            wl_dead = true;
            d.shutdown_now();
        }
        if !wl_dead {
            let _ = d.wl.queue.flush();
        }
        for ev in d.wl.take_events() {
            d.on_wl(ev);
        }
        if d.done {
            break;
        }
        // a dead connection stays readable forever: stop polling it or we spin
        let guard = if wl_dead { None } else { d.wl.queue.prepare_read() };
        let wl_fd = guard.as_ref().map(|g| g.connection_fd().as_raw_fd()).unwrap_or(-1);
        let mut fds = [
            libc::pollfd { fd: wl_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: d.bus.fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: sig, events: libc::POLLIN, revents: 0 },
        ];
        let timeout = match d.next_deadline() {
            None => -1, // nothing scheduled: sleep until something happens
            Some(t) => t.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32,
        };
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if n > 0 && fds[0].revents & libc::POLLIN != 0 {
            if let Some(g) = guard {
                let _ = g.read();
            }
        } else {
            drop(guard);
        }
        if n > 0 && fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            eprintln!("ztile: the compositor closed the connection");
            wl_dead = true;
            d.shutdown_now();
        }
        for m in d.bus.pump() {
            d.on_message(m);
        }
        if n > 0 && fds[2].revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 16];
            unsafe {
                libc::read(sig, buf.as_mut_ptr() as *mut _, buf.len());
            }
            d.shutdown();
        }
        d.tick();
        if d.done {
            break;
        }
    }
}
