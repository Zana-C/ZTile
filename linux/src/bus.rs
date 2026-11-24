//! A thin layer over libdbus (the `dbus` crate's low-level channel): our own event
//! loop polls its fd, so nothing here blocks except short calls at startup/exit.

use dbus::Message;
use dbus::channel::{BusType, Channel};
use std::time::Duration;

const SERVICE_NAME: &str = "io.github.ZanaC.ZTile";

/// The bus name. Debug builds only: ZTILE_DEV=1 uses a separate name (and KWin
/// script), so a development copy can run next to an installed one.
pub fn service() -> &'static str {
    if dev_mode() { "io.github.ZanaC.ZTile.Dev" } else { SERVICE_NAME }
}

pub fn dev_mode() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ZTILE_DEV").is_some()
}
pub const OBJECT: &str = "/ZTile";
pub const IFACE: &str = "io.github.ZanaC.ZTile";
const DBUS: &str = "org.freedesktop.DBus";
const SHORT: Duration = Duration::from_secs(5);

pub struct Bus {
    pub ch: Channel,
    kwin_owner: String,
}

impl Bus {
    pub fn connect() -> Result<Bus, String> {
        let mut ch = Channel::get_private(BusType::Session).map_err(|e| format!("no session bus ({e})"))?;
        ch.set_watch_enabled(true);
        Ok(Bus { ch, kwin_owner: String::new() })
    }

    pub fn fd(&self) -> i32 {
        self.ch.watch().fd
    }

    /// Blocking method call with a short timeout (startup, shutdown, CLI).
    pub fn call(&self, dest: &str, path: &str, iface: &str, method: &str, build: impl FnOnce(Message) -> Message) -> Result<Message, String> {
        let m = Message::new_method_call(dest, path, iface, method)?;
        self.ch.send_with_reply_and_block(build(m), SHORT).map_err(|e| e.message().unwrap_or("D-Bus error").to_string())
    }

    /// Fire-and-forget method call.
    pub fn call_noreply(&self, dest: &str, path: &str, iface: &str, method: &str, build: impl FnOnce(Message) -> Message) {
        if let Ok(m) = Message::new_method_call(dest, path, iface, method) {
            let mut m = build(m);
            m.set_no_reply(true);
            self.send(m);
        }
    }

    pub fn send(&self, m: Message) {
        let _ = self.ch.send(m);
        self.ch.flush();
    }

    /// Own `name` (no queueing). false if someone else has it.
    pub fn request_name(&self, name: &str) -> bool {
        const DO_NOT_QUEUE: u32 = 4;
        const PRIMARY_OWNER: u32 = 1;
        self.call(DBUS, "/org/freedesktop/DBus", DBUS, "RequestName", |m| m.append2(name, DO_NOT_QUEUE))
            .ok()
            .and_then(|r| r.get1::<u32>())
            == Some(PRIMARY_OWNER)
    }

    pub fn name_owner(&self, name: &str) -> Option<String> {
        self.call(DBUS, "/org/freedesktop/DBus", DBUS, "GetNameOwner", |m| m.append1(name)).ok()?.get1::<String>()
    }

    pub fn add_match(&self, rule: &str) {
        let _ = self.call(DBUS, "/org/freedesktop/DBus", DBUS, "AddMatch", |m| m.append1(rule));
    }

    /// Read what's waiting on the socket and return the queued messages.
    pub fn pump(&self) -> Vec<Message> {
        let _ = self.ch.read_write(Some(Duration::ZERO));
        let mut out = Vec::new();
        while let Some(m) = self.ch.pop_message() {
            out.push(m);
        }
        out
    }

    /// Was this call made by the KWin process (the owner of org.kde.KWin)?
    pub fn from_kwin(&mut self, m: &Message) -> bool {
        let Some(sender) = m.sender().map(|s| s.to_string()) else { return false };
        if !self.kwin_owner.is_empty() && sender == self.kwin_owner {
            return true;
        }
        self.kwin_owner = self.name_owner("org.kde.KWin").unwrap_or_default(); // KWin may have restarted
        !self.kwin_owner.is_empty() && sender == self.kwin_owner
    }

    pub fn reply_error(&self, call: &Message, name: &str, text: &str) {
        if let (Ok(n), Ok(t)) = (dbus::strings::ErrorName::new(name), std::ffi::CString::new(text)) {
            self.send(call.error(&n, &t));
        }
    }
}

/// Introspection data, so tools and Qt's QDBusInterface can talk to us.
pub fn introspect(path: &str) -> String {
    let head = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
                \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n";
    let std_ifaces = "<interface name=\"org.freedesktop.DBus.Introspectable\"><method name=\"Introspect\">\
        <arg type=\"s\" direction=\"out\"/></method></interface>\n\
        <interface name=\"org.freedesktop.DBus.Properties\">\
        <method name=\"Get\"><arg type=\"s\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/><arg type=\"v\" direction=\"out\"/></method>\
        <method name=\"GetAll\"><arg type=\"s\" direction=\"in\"/><arg type=\"a{sv}\" direction=\"out\"/></method></interface>\n";
    let body = match path {
        "/" => "<node name=\"ZTile\"/><node name=\"StatusNotifierItem\"/><node name=\"MenuBar\"/>".to_string(),
        OBJECT => format!(
            "<interface name=\"{IFACE}\">\
             <method name=\"poll\"><arg type=\"s\" direction=\"out\"/></method>\
             <method name=\"event\"><arg type=\"s\" direction=\"in\"/></method>\
             <method name=\"command\"><arg type=\"s\" direction=\"in\"/><arg type=\"s\" direction=\"out\"/></method>\
             <signal name=\"Changed\"/><signal name=\"ShowWindow\"/></interface>"
        ),
        crate::tray::ITEM_PATH => crate::tray::ITEM_XML.to_string(),
        crate::tray::MENU_PATH => crate::tray::MENU_XML.to_string(),
        _ => String::new(),
    };
    format!("{head}{std_ifaces}{body}\n</node>\n")
}
