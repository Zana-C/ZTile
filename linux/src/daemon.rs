//! The resident part of ZTile.
//!
//! The KWin script (kwin/contents/code/main.js) does everything that touches other
//! apps' windows. This process:
//! - loads that script into KWin and talks to it over D-Bus (a long poll for
//!   commands, an "event" method for what happens on the other side; both accepted
//!   from KWin only)
//! - owns ZTile's own little windows (group buttons, "+" cells, drop overlay)
//! - starts apps the script asks for, writes group changes back to saved layouts
//!   that opted in, serves the tray icon, and starts the settings window on demand

use crate::apps;
use crate::bus::{Bus, IFACE, OBJECT, introspect};
use crate::files;
use crate::icon;
use crate::store::{AppSpec, SavedLayout, Store};
use crate::tray::{MenuData, Tray, TrayAction};
use crate::wl::{Role, Wl, WlEvent};
use dbus::Message;
use dbus::message::MessageType;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use ztile_core::json::Json;
use ztile_core::layout::NRect;

pub const APP_ID: &str = "io.github.ZanaC.ZTile";
const PROXY_PREFIX: &str = "ztile-group-";
const SCRIPT_NAME: &str = "ztile";
/// answer the script's poll before D-Bus times it out (25 s)
const KEEPALIVE: Duration = Duration::from_secs(20);
const MAX_PAYLOAD: usize = 1_000_000;

struct Group {
    id: u32,
    name: String,
    saved: SavedLayout,
    proxy_app: String,
    proxy: Option<u32>,
    plus: Vec<(usize, u32)>,
    /// cells from the script's last state: [{win: {id, app}|null, spec}]
    cells: Option<Vec<Json>>,
    confirmed: bool,
    icon_key: Option<Vec<Option<String>>>,
    desktop_file: Option<PathBuf>,
    icon_file: Option<PathBuf>,
}

impl Group {
    fn ncells(&self) -> Vec<NRect> {
        self.saved.tree.cells()
    }

    /// App id per cell as shown right now (live window, else nothing).
    fn cell_apps(&self) -> Vec<Option<String>> {
        match &self.cells {
            None => self.saved.apps.iter().map(|a| a.as_ref().map(|a| a.app.clone())).collect(),
            Some(cells) => cells
                .iter()
                .map(|c| c.get("win").and_then(|w| w.get("app")).and_then(Json::as_str).map(str::to_string))
                .collect(),
        }
    }

    /// What each cell belongs to (survives the app closing).
    fn specs(&self) -> Vec<Json> {
        match &self.cells {
            None => self.saved.apps.iter().map(|a| a.as_ref().map_or(Json::Null, AppSpec::engine_json)).collect(),
            Some(cells) => cells.iter().map(|c| c.get("spec").cloned().unwrap_or(Json::Null)).collect(),
        }
    }
}

enum Job {
    Sycoca,
    Other,
}

pub struct Daemon {
    pub bus: Bus,
    pub wl: Wl,
    tray: Tray,
    polls: Vec<Message>,
    poll_since: Option<Instant>,
    queue: Vec<Json>,
    groups: Vec<Group>,
    next_gid: u32,
    outputs: Vec<Json>,
    overlay: Option<u32>,
    connected: bool,
    win_reqs: Vec<(u64, Message)>,
    next_req: u64,
    children: Vec<(Child, Job)>,
    sycoca_again: bool,
    ui: Option<Child>,
    script: PathBuf,
    ui_dir: Option<PathBuf>,
    /// shutting down: finish at this time (the script gets a moment to give windows back)
    pub quit_at: Option<Instant>,
    pub done: bool,
}

fn s(v: &str) -> Json {
    Json::str(v)
}

fn num(v: f64) -> Json {
    Json::Num(v)
}

fn get_u32(j: &Json, k: &str) -> Option<u32> {
    j.get(k).and_then(Json::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v <= u32::MAX as f64).map(|v| v as u32)
}

impl Daemon {
    pub fn new(bus: Bus, wl: Wl, script: PathBuf, ui_dir: Option<PathBuf>) -> Daemon {
        Daemon {
            bus,
            wl,
            tray: Tray::new(),
            polls: Vec::new(),
            poll_since: None,
            queue: Vec::new(),
            groups: Vec::new(),
            next_gid: 1,
            outputs: Vec::new(),
            overlay: None,
            connected: false,
            win_reqs: Vec::new(),
            next_req: 1,
            children: Vec::new(),
            sycoca_again: false,
            ui: None,
            script,
            ui_dir,
            quit_at: None,
            done: false,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if !crate::bus::dev_mode() {
            self.cleanup_group_files(); // a development copy leaves the real one's files alone
        }
        self.bus.add_match("type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.kde.StatusNotifierWatcher'");
        self.tray.register(&self.bus);
        self.load_script()
    }

    // ------------------------------------------------------------ KWin script

    fn script_name() -> &'static str {
        if crate::bus::dev_mode() { "ztile-dev" } else { SCRIPT_NAME }
    }

    fn load_script(&mut self) -> Result<(), String> {
        let mut path = self.script.to_string_lossy().into_owned();
        if crate::bus::dev_mode() {
            // the script calls us by name: give the development copy its own
            let src = std::fs::read_to_string(&self.script).map_err(|e| e.to_string())?;
            let src = src.replacen(
                "const SERVICE = \"io.github.ZanaC.ZTile\";",
                &format!("const SERVICE = \"{}\";", crate::bus::service()),
                1,
            );
            let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or("no XDG_RUNTIME_DIR")?;
            let p = dir.join("ztile-dev-main.js");
            files::write_atomic(&p, src.as_bytes(), 0o600).map_err(|e| e.to_string())?;
            path = p.to_string_lossy().into_owned();
        }
        let sc = |m: &str| ("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", m.to_string());
        let (d, p, i, _) = sc("");
        let _ = self.bus.call(d, p, i, "unloadScript", |m| m.append1(Self::script_name()));
        let r = self
            .bus
            .call(d, p, i, "loadScript", |m| m.append2(path.as_str(), Self::script_name()))
            .map_err(|e| format!("KWin's scripting interface isn't available ({e}). ZTile needs KDE Plasma 6."))?;
        let id: i32 = r.get1().unwrap_or(-1);
        if id < 0 {
            return Err("KWin refused to load ZTile's script.".into());
        }
        self.bus
            .call("org.kde.KWin", &format!("/Scripting/Script{id}"), "org.kde.kwin.Script", "run", |m| m)
            .map(|_| ())
    }

    fn unload_script(&self) {
        let _ = self.bus.call("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", |m| m.append1(Self::script_name()));
    }

    fn send_cmd(&mut self, cmd: Json) {
        self.queue.push(cmd);
        self.flush_polls();
    }

    fn flush_polls(&mut self) {
        while !self.queue.is_empty() && !self.polls.is_empty() {
            let call = self.polls.remove(0);
            let batch = Json::Arr(std::mem::take(&mut self.queue));
            self.bus.send(call.method_return().append1(batch.compact()));
        }
        if self.polls.is_empty() {
            self.poll_since = None;
        }
    }

    // ------------------------------------------------------------ timing

    /// When the main loop must wake up next.
    pub fn next_deadline(&self) -> Option<Instant> {
        let mut d = self.poll_since.map(|t| t + KEEPALIVE);
        if let Some(q) = self.quit_at {
            d = Some(d.map_or(q, |x| x.min(q)));
        }
        if !self.children.is_empty() || self.ui.is_some() {
            // reap started processes (kioclient, kbuildsycoca, the settings window)
            let c = Instant::now() + Duration::from_millis(if self.children.is_empty() { 1000 } else { 100 });
            d = Some(d.map_or(c, |x| x.min(c)));
        }
        d
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.poll_since.is_some_and(|t| now >= t + KEEPALIVE) {
            for call in std::mem::take(&mut self.polls) {
                self.bus.send(call.method_return().append1("[]"));
            }
            self.poll_since = None;
        }
        let mut sycoca_done = false;
        self.children.retain_mut(|(c, job)| match c.try_wait() {
            Ok(Some(_)) | Err(_) => {
                if matches!(job, Job::Sycoca) {
                    sycoca_done = true;
                }
                false
            }
            Ok(None) => true,
        });
        if sycoca_done {
            if self.sycoca_again {
                self.sycoca_again = false;
                self.run_sycoca();
            } else {
                self.ensure_proxies();
            }
        }
        if let Some(ui) = &mut self.ui {
            if !matches!(ui.try_wait(), Ok(None)) {
                self.ui = None; // the settings window closed: nothing of it stays in memory
            }
        }
        if self.quit_at.is_some_and(|q| now >= q) {
            self.finish();
        }
    }

    // ------------------------------------------------------------ D-Bus messages

    pub fn on_message(&mut self, m: Message) {
        match m.msg_type() {
            MessageType::MethodCall => self.on_call(m),
            MessageType::Signal => {
                if m.member().as_deref() == Some("NameOwnerChanged") {
                    if let Ok((name, _, new)) = m.read3::<&str, &str, &str>() {
                        if name == "org.kde.StatusNotifierWatcher" && !new.is_empty() {
                            self.tray.register(&self.bus); // plasmashell restarted
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_call(&mut self, m: Message) {
        let path = m.path().map(|p| p.to_string()).unwrap_or_default();
        let iface = m.interface().map(|i| i.to_string()).unwrap_or_default();
        let member = m.member().map(|x| x.to_string()).unwrap_or_default();
        if iface == "org.freedesktop.DBus.Introspectable" && member == "Introspect" {
            self.bus.send(m.method_return().append1(introspect(&path)));
            return;
        }
        if iface == "org.freedesktop.DBus.Peer" {
            self.bus.send(m.method_return());
            return;
        }
        if path == crate::tray::ITEM_PATH || path == crate::tray::MENU_PATH {
            let md = self.menu_data();
            if let Some(Some(a)) = self.tray.handle(&self.bus, &m, &md) {
                self.on_tray(a);
            }
            return;
        }
        if path != OBJECT || (iface != IFACE && !iface.is_empty()) {
            self.bus.reply_error(&m, "org.freedesktop.DBus.Error.UnknownObject", "unknown object");
            return;
        }
        match member.as_str() {
            "poll" => {
                if !self.bus.from_kwin(&m) {
                    self.bus.reply_error(&m, "org.freedesktop.DBus.Error.AccessDenied", "KWin only");
                    return;
                }
                // one script polls at a time; a stale one (script reloaded) is answered now
                while self.polls.len() >= 2 {
                    let old = self.polls.remove(0);
                    self.bus.send(old.method_return().append1("[]"));
                }
                self.polls.push(m);
                self.poll_since.get_or_insert_with(Instant::now);
                self.flush_polls();
            }
            "event" => {
                // Only KWin (our script) may report window events: another program on
                // the session bus must not make us start apps or rewrite layouts.
                let ok = self.bus.from_kwin(&m);
                let payload = m.read1::<&str>().ok().map(str::to_string);
                self.bus.send(m.method_return());
                if let (true, Some(p)) = (ok, payload) {
                    if p.len() <= MAX_PAYLOAD {
                        if let Ok(ev) = Json::parse(&p) {
                            self.on_script_event(&ev);
                        }
                    }
                }
            }
            "command" => {
                let args: Option<Vec<String>> = m.read1::<&str>().ok().filter(|p| p.len() <= 64 * 1024).and_then(|p| {
                    let j = Json::parse(p).ok()?;
                    let a = j.as_arr()?;
                    if a.len() > 32 {
                        return None;
                    }
                    a.iter().map(|x| x.as_str().map(str::to_string)).collect()
                });
                match args {
                    None => self.bus.send(m.method_return().append1("bad request")),
                    Some(a) => {
                        if let Some(r) = self.command(&a, &m) {
                            self.bus.send(m.method_return().append1(r));
                        }
                    }
                }
            }
            _ => self.bus.reply_error(&m, "org.freedesktop.DBus.Error.UnknownMethod", "unknown method"),
        }
    }

    fn emit_changed(&mut self) {
        if let Ok(sig) = Message::new_signal(OBJECT, IFACE, "Changed") {
            self.bus.send(sig);
        }
        let d = self.menu_data();
        self.tray.changed(&self.bus, &d);
    }

    // ------------------------------------------------------------ commands (CLI, settings window)

    /// Some(text) = reply now; None = the reply is sent later.
    fn command(&mut self, a: &[String], call: &Message) -> Option<String> {
        let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
        let n = |i: usize| arg(i).parse::<u32>().ok();
        Some(match arg(0) {
            "" | "show" => {
                self.show_ui(&[]);
                String::new()
            }
            "open" => {
                let st = Store::load();
                match st.get(arg(1)) {
                    Some(l) => {
                        let l = l.clone();
                        self.open_saved(&l);
                        format!("Opened \"{}\".", l.name)
                    }
                    None => format!("No saved layout named \"{}\".", arg(1)),
                }
            }
            "close" => {
                if self.find(arg(1)).is_none() {
                    format!("\"{}\" isn't open.", arg(1))
                } else {
                    self.close_named(arg(1));
                    format!("Closed \"{}\".", arg(1))
                }
            }
            "quit" => {
                self.shutdown();
                "Quitting.".into()
            }
            "list" => self.list_text(),
            "state" => self.state_json().compact(),
            "windows" => {
                if !self.connected {
                    return Some("[]".into());
                }
                let req = self.next_req;
                self.next_req += 1;
                self.win_reqs.push((req, call.method_return()));
                self.send_cmd(Json::obj(vec![("cmd", s("windows")), ("req", num(req as f64))]));
                return None;
            }
            "cell" => {
                let spec = n(1)
                    .zip(n(2))
                    .and_then(|(g, c)| self.groups.iter().find(|x| x.id == g).map(|x| x.specs().get(c as usize).cloned()))
                    .flatten()
                    .unwrap_or(Json::Null);
                let name = spec.get("app").and_then(Json::as_str).map(apps::display_name).unwrap_or_default();
                Json::obj(vec![("spec", spec), ("name", s(&name))]).compact()
            }
            "fill" => {
                if let (Some(g), Some(c)) = (n(1), n(2)) {
                    let win = arg(3).chars().take(64).collect::<String>();
                    self.send_cmd(Json::obj(vec![("cmd", s("fill")), ("id", num(g as f64)), ("cell", num(c as f64)), ("win", s(&win))]));
                }
                String::new()
            }
            "launch" => {
                if let (Some(g), Some(c)) = (n(1), n(2)) {
                    let spec = Json::obj(vec![("app", s(arg(3))), ("window", s(""))]);
                    self.send_cmd(Json::obj(vec![
                        ("cmd", s("launch")),
                        ("id", num(g as f64)),
                        ("cell", num(c as f64)),
                        ("spec", spec),
                        ("user", Json::Bool(true)),
                    ]));
                }
                String::new()
            }
            _ => "unknown command".into(),
        })
    }

    fn state_json(&self) -> Json {
        Json::obj(vec![
            ("outputs", Json::Arr(self.outputs.clone())),
            ("open", Json::Arr(self.groups.iter().filter(|g| !g.name.is_empty()).map(|g| s(&g.name)).collect())),
            ("connected", Json::Bool(self.connected)),
        ])
    }

    fn list_text(&self) -> String {
        let mut out = vec!["Displays:".to_string()];
        for (i, o) in self.outputs.iter().enumerate() {
            out.push(format!(
                "  [{}] {}  {}x{}  {}",
                i + 1,
                o.str_or_empty("name"),
                get_u32(o, "width").unwrap_or(0),
                get_u32(o, "height").unwrap_or(0),
                o.str_or_empty("label")
            ));
        }
        out.push("\nOpen groups:".into());
        for g in &self.groups {
            let apps: Vec<String> = g.cell_apps().iter().map(|a| a.as_deref().map(apps::display_name).unwrap_or("+".into())).collect();
            out.push(format!("  {}: {}", if g.name.is_empty() { g.id.to_string() } else { g.name.clone() }, apps.join(" | ")));
        }
        if self.groups.is_empty() {
            out.push("  (none)".into());
        }
        out.join("\n")
    }

    // ------------------------------------------------------------ tray

    fn menu_data(&self) -> MenuData {
        let open: Vec<String> = self.groups.iter().map(|g| g.name.to_lowercase()).collect();
        MenuData {
            layouts: Store::load().layouts.iter().map(|l| (l.name.clone(), open.contains(&l.name.to_lowercase()))).collect(),
            autostart: autostart_file().is_file(),
        }
    }

    fn on_tray(&mut self, a: TrayAction) {
        match a {
            TrayAction::Show => self.show_ui(&[]),
            TrayAction::OpenLayout(name) => {
                if let Some(l) = Store::load().get(&name).cloned() {
                    self.open_saved(&l);
                }
            }
            TrayAction::ToggleAutostart => {
                set_autostart(!autostart_file().is_file());
                let d = self.menu_data();
                self.tray.changed(&self.bus, &d);
            }
            TrayAction::Quit => self.shutdown(),
        }
    }

    // ------------------------------------------------------------ settings window

    pub fn show_window(&mut self) {
        self.show_ui(&[]);
    }

    /// The settings window is a separate process that runs only while it's open.
    fn show_ui(&mut self, extra: &[String]) {
        if extra.is_empty() && self.ui.is_some() {
            if let Ok(sig) = Message::new_signal(OBJECT, IFACE, "ShowWindow") {
                self.bus.send(sig);
            }
            return;
        }
        let Some(dir) = self.ui_dir.clone() else {
            self.notify("The settings window isn't installed (it needs Python 3 with PyQt6).");
            return;
        };
        let child = Command::new("/usr/bin/python3")
            .arg("-P")
            .arg("-m")
            .arg("ztile_ui")
            .args(extra)
            .env("PYTHONPATH", &dir)
            .stdin(Stdio::null())
            .spawn();
        match child {
            Ok(c) if extra.is_empty() => self.ui = Some(c),
            Ok(c) => self.children.push((c, Job::Other)),
            Err(e) => self.notify(&format!("Couldn't open the settings window ({e}).")),
        }
    }

    fn notify(&self, text: &str) {
        let esc = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        self.bus.call_noreply(
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            |m| {
                m.append3("ZTile", 0u32, APP_ID)
                    .append3("ZTile", esc, Vec::<String>::new())
                    .append2(dbus::arg::PropMap::new(), -1i32)
            },
        );
    }

    // ------------------------------------------------------------ script events

    fn on_script_event(&mut self, ev: &Json) {
        match ev.get("ev").and_then(Json::as_str).unwrap_or("") {
            "hello" => {
                self.connected = true;
                // config first: commands queued before the script was up come after it
                self.queue.insert(0, Json::obj(vec![("cmd", s("config")), ("pid", num(std::process::id() as f64))]));
                self.flush_polls();
                if self.overlay.is_none() {
                    self.overlay = Some(self.wl.create(Role::Overlay, APP_ID, "ZTile overlay"));
                }
                // KWin (or the script) restarted: hand it the groups it knew again
                let reopen: Vec<Json> =
                    self.groups.iter().filter(|g| g.confirmed).map(|g| Self::group_cmd(g, Json::Arr(g.specs()))).collect();
                for g in reopen {
                    self.send_cmd(Json::obj(vec![("cmd", s("open")), ("group", g), ("launch", Json::Bool(false))]));
                }
            }
            "outputs" => {
                if let Some(list) = ev.get("list").and_then(Json::as_arr) {
                    let mut l: Vec<Json> = list.to_vec();
                    // numbered left to right, like the displays on the desk
                    let key = |o: &Json| (o.get("x").and_then(Json::as_f64).unwrap_or(0.0), o.get("y").and_then(Json::as_f64).unwrap_or(0.0));
                    l.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
                    self.outputs = l;
                    self.emit_changed();
                }
            }
            "state" => self.on_state(ev.get("groups").and_then(Json::as_arr).unwrap_or(&[])),
            "launch" => {
                let app = ev.get("spec").and_then(|s| s.get("app")).and_then(Json::as_str).unwrap_or("");
                if !app.is_empty() {
                    match apps::launch(app) {
                        Some(c) => self.children.push((c, Job::Other)),
                        None => self.notify(&format!("Couldn't start {}.", apps::display_name(app))),
                    }
                }
            }
            "overlay" => {
                let color = match ev.get("kind").and_then(Json::as_str) {
                    Some("move") => [0, 120, 212, 90],
                    Some("stay") => [128, 128, 128, 90],
                    Some("leave") => [232, 17, 35, 90],
                    _ => [0, 0, 0, 0],
                };
                if let Some(o) = self.overlay {
                    self.wl.set_color(o, color);
                }
            }
            "windows" => {
                let req = ev.get("req").and_then(Json::as_f64).unwrap_or(-1.0) as u64;
                if let Some(i) = self.win_reqs.iter().position(|(r, _)| *r == req) {
                    let (_, reply) = self.win_reqs.remove(i);
                    let list = ev.get("list").cloned().unwrap_or(Json::Arr(Vec::new()));
                    self.bus.send(reply.append1(list.compact()));
                }
            }
            _ => {}
        }
    }

    fn on_state(&mut self, states: &[Json]) {
        let mut seen = Vec::new();
        let mut user_changed = Vec::new();
        for st in states {
            let Some(id) = get_u32(st, "id") else { continue };
            let Some(cells) = st.get("cells").and_then(Json::as_arr) else { continue };
            let Some(gi) = self.groups.iter().position(|g| g.id == id) else { continue };
            seen.push(id);
            let g = &mut self.groups[gi];
            g.cells = Some(cells.to_vec());
            g.confirmed = true;
            if st.get("user").and_then(Json::as_bool) == Some(true) {
                user_changed.push(id);
            }
            self.sync_plus(gi);
            self.sync_proxy(gi);
        }
        let gone: Vec<u32> = self.groups.iter().filter(|g| g.confirmed && !seen.contains(&g.id)).map(|g| g.id).collect();
        for id in gone {
            self.forget(id); // dissolved on the KWin side
        }
        for id in user_changed {
            self.persist(id);
        }
        self.emit_changed();
    }

    fn sync_plus(&mut self, gi: usize) {
        let g = &self.groups[gi];
        let cells = g.cells.clone().unwrap_or_default();
        let gid = g.id;
        let mut plus = std::mem::take(&mut self.groups[gi].plus);
        for (i, c) in cells.iter().enumerate() {
            let need = matches!(c.get("win"), None | Some(Json::Null));
            let has = plus.iter().position(|(ci, _)| *ci == i);
            match (need, has) {
                (true, None) => plus.push((i, self.wl.create(Role::Plus(gid, i), APP_ID, &format!("ZTile cell {gid} {i}")))),
                (false, Some(k)) => {
                    let (_, w) = plus.remove(k);
                    self.wl.destroy(w);
                }
                _ => {}
            }
        }
        plus.retain(|(ci, w)| {
            if *ci >= cells.len() {
                self.wl.destroy(*w);
                false
            } else {
                true
            }
        });
        self.groups[gi].plus = plus;
    }

    fn proxy_title(g: &Group) -> String {
        let names: Vec<String> = g.cell_apps().iter().map(|a| a.as_deref().map(apps::display_name).unwrap_or("+".into())).collect();
        format!("{} \u{b7} {}", if g.name.is_empty() { "ZTile" } else { &g.name }, names.join(" | "))
    }

    fn sync_proxy(&mut self, gi: usize) {
        let key = self.groups[gi].cell_apps();
        if self.groups[gi].icon_key.as_ref() != Some(&key) {
            self.groups[gi].icon_key = Some(key);
            if let Some(p) = self.groups[gi].proxy {
                let t = Self::proxy_title(&self.groups[gi]);
                self.wl.set_title(p, &t);
            }
            self.write_group_files(gi);
        }
    }

    /// Write the group's apps back to its saved layout, if it asks for that.
    fn persist(&mut self, id: u32) {
        let Some(g) = self.groups.iter_mut().find(|g| g.id == id) else { return };
        if g.name.is_empty() || !g.saved.keep_changes {
            return;
        }
        let specs: Vec<Option<AppSpec>> = g
            .specs()
            .iter()
            .map(|s| {
                let app = s.get("app").and_then(Json::as_str).filter(|a| !a.is_empty())?;
                Some(AppSpec { app: app.into(), name: apps::display_name(app), window: s.str_or_empty("window") })
            })
            .collect();
        let mut st = Store::load();
        let Some(l) = st.get_mut(&g.name) else { return };
        if l.apps.len() != specs.len() || l.apps == specs {
            return;
        }
        l.apps = specs.clone();
        g.saved.apps = specs;
        if let Err(e) = st.save() {
            self.notify(&format!("Couldn't save: {e}"));
        }
    }

    // ------------------------------------------------------------ taskbar button files

    fn write_group_files(&mut self, gi: usize) {
        let g = &self.groups[gi];
        let key = g.icon_key.clone().unwrap_or_else(|| g.cell_apps());
        let icons: Vec<Option<apps::Image>> = key.iter().map(|a| a.as_deref().and_then(|a| apps::icon(a, 64))).collect();
        let png = icon::composite(&g.ncells(), &icons, 128).png();
        let cache = files::cache_home().join("ztile");
        let _ = files::private_dir(&cache);
        let mut h: u64 = 0xcbf29ce484222325; // FNV-1a over the picture: a new file name per icon
        for b in &png {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        let icon_file = cache.join("icons").join(format!("{}-{:012x}.png", g.proxy_app, h & 0xffff_ffff_ffff));
        if !icon_file.exists() && files::write_atomic(&icon_file, &png, 0o644).is_err() {
            return;
        }
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let name = if g.name.is_empty() { "ZTile group".to_string() } else { g.name.clone() };
        let desktop = files::data_home().join("applications").join(format!("{}.desktop", g.proxy_app));
        // The layout name comes from a file: it must not be able to add keys
        // (a newline followed by Exec=...) to an entry Plasma may run.
        let text = format!(
            "[Desktop Entry]\nType=Application\nName={}\nComment=ZTile group\nIcon={}\nExec={}\nNoDisplay=true\nStartupWMClass={}\n",
            files::desktop_value(&name),
            files::desktop_value(&icon_file.to_string_lossy()),
            if exe.is_empty() { "true".to_string() } else { files::exec_arg(&exe) },
            g.proxy_app
        );
        if files::write_atomic(&desktop, text.as_bytes(), 0o644).is_err() {
            return;
        }
        let g = &mut self.groups[gi];
        if let Some(old) = g.icon_file.replace(icon_file.clone()) {
            if old != icon_file {
                let _ = std::fs::remove_file(old);
            }
        }
        g.desktop_file = Some(desktop);
        self.run_sycoca();
    }

    /// Tell Plasma about new/changed .desktop files (~0.1 s); proxies follow.
    fn run_sycoca(&mut self) {
        if self.children.iter().any(|(_, j)| matches!(j, Job::Sycoca)) {
            self.sycoca_again = true;
            return;
        }
        match Command::new("kbuildsycoca6").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(c) => self.children.push((c, Job::Sycoca)),
            Err(_) => self.ensure_proxies(),
        }
    }

    fn ensure_proxies(&mut self) {
        if self.quit_at.is_some() {
            return;
        }
        for gi in 0..self.groups.len() {
            if self.groups[gi].proxy.is_none() && self.groups[gi].desktop_file.is_some() {
                let title = Self::proxy_title(&self.groups[gi]);
                let (gid, app) = (self.groups[gi].id, self.groups[gi].proxy_app.clone());
                self.groups[gi].proxy = Some(self.wl.create(Role::Proxy(gid), &app, &title));
            }
        }
    }

    fn cleanup_group_files(&mut self) {
        let mut removed = false;
        if let Ok(rd) = std::fs::read_dir(files::data_home().join("applications")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with(PROXY_PREFIX) && n.ends_with(".desktop") {
                    let _ = std::fs::remove_file(e.path());
                    removed = true;
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(files::cache_home().join("ztile").join("icons")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(PROXY_PREFIX) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        if removed {
            self.run_sycoca();
        }
    }

    // ------------------------------------------------------------ groups

    fn group_cmd(g: &Group, specs: Json) -> Json {
        let l = &g.saved;
        Json::obj(vec![
            ("id", num(g.id as f64)),
            ("name", s(&g.name)),
            ("proxyApp", s(&g.proxy_app)),
            (
                "monitor",
                Json::obj(vec![
                    ("name", s(&l.monitor.name)),
                    ("width", num(l.monitor.width as f64)),
                    ("height", num(l.monitor.height as f64)),
                ]),
            ),
            (
                "cells",
                Json::Arr(
                    g.ncells()
                        .iter()
                        .map(|c| Json::Arr(vec![num(c.x0 as f64), num(c.y0 as f64), num(c.x1 as f64), num(c.y1 as f64)]))
                        .collect(),
                ),
            ),
            ("gap", num(l.gap as f64)),
            ("borderless", Json::Bool(l.borderless)),
            ("specs", specs),
        ])
    }

    fn find(&self, name: &str) -> Option<u32> {
        let n = name.to_lowercase();
        self.groups.iter().find(|g| !g.name.is_empty() && g.name.to_lowercase() == n).map(|g| g.id)
    }

    /// Open a saved layout, or bring it up if it's open already.
    pub fn open_saved(&mut self, l: &SavedLayout) {
        if let Some(id) = self.find(&l.name) {
            self.send_cmd(Json::obj(vec![("cmd", s("show")), ("id", num(id as f64))]));
            return;
        }
        let gid = self.next_gid;
        self.next_gid += 1;
        let g = Group {
            id: gid,
            name: l.name.clone(),
            saved: l.clone(),
            proxy_app: format!("{PROXY_PREFIX}{}", slug(&l.name, gid)),
            proxy: None,
            plus: Vec::new(),
            cells: None,
            confirmed: false,
            icon_key: Some(l.apps.iter().map(|a| a.as_ref().map(|a| a.app.clone())).collect()),
            desktop_file: None,
            icon_file: None,
        };
        let specs = Json::Arr(l.apps.iter().map(|a| a.as_ref().map_or(Json::Null, AppSpec::engine_json)).collect());
        let cmd = Json::obj(vec![("cmd", s("open")), ("group", Self::group_cmd(&g, specs)), ("launch", Json::Bool(true))]);
        self.groups.push(g);
        self.write_group_files(self.groups.len() - 1);
        self.send_cmd(cmd);
        self.emit_changed();
    }

    fn close_group(&mut self, id: u32) {
        if self.groups.iter().any(|g| g.id == id) {
            self.send_cmd(Json::obj(vec![("cmd", s("close")), ("id", num(id as f64))]));
            self.forget(id);
            self.emit_changed();
        }
    }

    fn close_named(&mut self, name: &str) {
        if let Some(id) = self.find(name) {
            self.close_group(id);
        }
    }

    fn forget(&mut self, id: u32) {
        let Some(i) = self.groups.iter().position(|g| g.id == id) else { return };
        let g = self.groups.remove(i);
        for (_, w) in g.plus {
            self.wl.destroy(w);
        }
        if let Some(p) = g.proxy {
            self.wl.destroy(p);
        }
        if let Some(f) = g.desktop_file {
            let _ = std::fs::remove_file(f);
        }
        if let Some(f) = g.icon_file {
            let _ = std::fs::remove_file(f);
        }
    }

    // ------------------------------------------------------------ our windows

    pub fn on_wl(&mut self, ev: WlEvent) {
        match ev {
            // "Close" on the group's taskbar button dissolves the group
            WlEvent::Closed(Role::Proxy(gid)) => self.close_group(gid),
            // A "+" cell or the overlay must close too when asked: at logout Plasma asks
            // every window to close and waits for them, so ignoring it blocks the logout.
            WlEvent::Closed(Role::Plus(gid, _)) => self.close_group(gid),
            WlEvent::Closed(Role::Overlay) => {
                if let Some(o) = self.overlay.take() {
                    self.wl.destroy(o);
                }
            }
            WlEvent::Clicked(Role::Plus(gid, cell)) => self.show_ui(&["--cell".into(), gid.to_string(), cell.to_string()]),
            _ => {}
        }
    }

    // ------------------------------------------------------------ exit

    /// Dissolve every group (the script gives the windows back), then exit.
    pub fn shutdown(&mut self) {
        if self.quit_at.is_some() {
            return;
        }
        self.send_cmd(Json::obj(vec![("cmd", s("closeAll"))]));
        let wait = if self.connected { 300 } else { 0 };
        self.quit_at = Some(Instant::now() + Duration::from_millis(wait));
    }

    /// The compositor is gone (logout): nobody is left to hand windows back to, exit now.
    pub fn shutdown_now(&mut self) {
        self.shutdown();
        self.quit_at = Some(Instant::now());
    }

    fn finish(&mut self) {
        let ids: Vec<u32> = self.groups.iter().map(|g| g.id).collect();
        for id in ids {
            self.forget(id);
        }
        if let Some(o) = self.overlay.take() {
            self.wl.destroy(o);
        }
        let _ = self.wl.queue.flush();
        self.unload_script();
        if let Some(mut ui) = self.ui.take() {
            let _ = ui.kill();
            let _ = ui.wait();
        }
        // don't wait for it: during a logout it can take long, and the session waits for us
        let _ = Command::new("kbuildsycoca6").stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        self.done = true;
    }
}

/// Stable, valid app id part for a group name ("İş" and "Is" stay different).
fn slug(name: &str, fallback: u32) -> String {
    if name.is_empty() {
        return fallback.to_string();
    }
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else {
            s.push_str(&format!("_{:x}", c as u32));
        }
    }
    s.chars().take(100).collect()
}

pub fn autostart_file() -> PathBuf {
    files::config_home().join("autostart").join(format!("{APP_ID}.desktop"))
}

fn set_autostart(on: bool) {
    let f = autostart_file();
    if !on {
        let _ = std::fs::remove_file(f);
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=ZTile\nComment=Window layout groups\nExec={} --tray\nIcon={APP_ID}\nX-KDE-autostart-phase=2\n",
        files::exec_arg(&exe.to_string_lossy())
    );
    let _ = files::write_atomic(&f, text.as_bytes(), 0o644);
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs_are_valid_and_distinct() {
        assert_eq!(slug("Chat", 1), "chat");
        assert_eq!(slug("", 7), "7");
        assert_ne!(slug("İş", 1), slug("Is", 1));
        assert!(slug("İş Yeri", 1).chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }
}
