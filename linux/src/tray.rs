//! Tray icon: a StatusNotifierItem with a com.canonical.dbusmenu menu, served
//! straight over D-Bus (what Plasma's system tray speaks; no toolkit needed).

use crate::bus::Bus;
use dbus::Message;
use dbus::arg::{PropMap, RefArg, Variant};

pub const ITEM_PATH: &str = "/StatusNotifierItem";
pub const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const PROPS: &str = "org.freedesktop.DBus.Properties";

pub const ITEM_XML: &str = "<interface name=\"org.kde.StatusNotifierItem\">\
    <property name=\"Category\" type=\"s\" access=\"read\"/><property name=\"Id\" type=\"s\" access=\"read\"/>\
    <property name=\"Title\" type=\"s\" access=\"read\"/><property name=\"Status\" type=\"s\" access=\"read\"/>\
    <property name=\"IconName\" type=\"s\" access=\"read\"/><property name=\"IconPixmap\" type=\"a(iiay)\" access=\"read\"/>\
    <property name=\"ToolTip\" type=\"(sa(iiay)ss)\" access=\"read\"/><property name=\"ItemIsMenu\" type=\"b\" access=\"read\"/>\
    <property name=\"Menu\" type=\"o\" access=\"read\"/>\
    <method name=\"Activate\"><arg type=\"i\" direction=\"in\"/><arg type=\"i\" direction=\"in\"/></method>\
    <method name=\"SecondaryActivate\"><arg type=\"i\" direction=\"in\"/><arg type=\"i\" direction=\"in\"/></method>\
    <method name=\"ContextMenu\"><arg type=\"i\" direction=\"in\"/><arg type=\"i\" direction=\"in\"/></method>\
    <method name=\"Scroll\"><arg type=\"i\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
    <signal name=\"NewIcon\"/><signal name=\"NewToolTip\"/><signal name=\"NewStatus\"><arg type=\"s\"/></signal></interface>";

pub const MENU_XML: &str = "<interface name=\"com.canonical.dbusmenu\">\
    <property name=\"Version\" type=\"u\" access=\"read\"/><property name=\"TextDirection\" type=\"s\" access=\"read\"/>\
    <property name=\"Status\" type=\"s\" access=\"read\"/><property name=\"IconThemePath\" type=\"as\" access=\"read\"/>\
    <method name=\"GetLayout\"><arg type=\"i\" direction=\"in\"/><arg type=\"i\" direction=\"in\"/><arg type=\"as\" direction=\"in\"/>\
    <arg type=\"u\" direction=\"out\"/><arg type=\"(ia{sv}av)\" direction=\"out\"/></method>\
    <method name=\"GetGroupProperties\"><arg type=\"ai\" direction=\"in\"/><arg type=\"as\" direction=\"in\"/><arg type=\"a(ia{sv})\" direction=\"out\"/></method>\
    <method name=\"GetProperty\"><arg type=\"i\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/><arg type=\"v\" direction=\"out\"/></method>\
    <method name=\"Event\"><arg type=\"i\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/><arg type=\"v\" direction=\"in\"/><arg type=\"u\" direction=\"in\"/></method>\
    <method name=\"EventGroup\"><arg type=\"a(isvu)\" direction=\"in\"/><arg type=\"ai\" direction=\"out\"/></method>\
    <method name=\"AboutToShow\"><arg type=\"i\" direction=\"in\"/><arg type=\"b\" direction=\"out\"/></method>\
    <method name=\"AboutToShowGroup\"><arg type=\"ai\" direction=\"in\"/><arg type=\"ai\" direction=\"out\"/><arg type=\"ai\" direction=\"out\"/></method>\
    <signal name=\"LayoutUpdated\"><arg type=\"u\"/><arg type=\"i\"/></signal></interface>";

pub enum TrayAction {
    Show,
    OpenLayout(String),
    ToggleAutostart,
    Quit,
}

enum Kind {
    Normal,
    Separator,
    Disabled,
    Check(bool),
}

struct Item {
    id: i32,
    label: String,
    kind: Kind,
    action: Option<TrayActionKey>,
}

#[derive(Clone)]
enum TrayActionKey {
    Show,
    Open(String),
    Autostart,
    Quit,
}

/// What the menu shows: saved layouts (name, open?) and the autostart state.
pub struct MenuData {
    pub layouts: Vec<(String, bool)>,
    pub autostart: bool,
}

pub struct Tray {
    revision: u32,
    items: Vec<Item>,
    pixmaps: Vec<(i32, i32, Vec<u8>)>,
}

fn var<T: RefArg + 'static>(v: T) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(v))
}

impl Tray {
    pub fn new() -> Tray {
        let pixmaps = [16u32, 22, 32, 48, 64]
            .iter()
            .map(|&s| (s as i32, s as i32, crate::icon::app_icon(s).to_argb_network()))
            .collect();
        Tray { revision: 1, items: Vec::new(), pixmaps }
    }

    /// Announce the item to Plasma's tray (again after plasmashell restarts).
    pub fn register(&self, bus: &Bus) {
        let me = bus.ch.unique_name().unwrap_or_default().to_string();
        bus.call_noreply(
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
            "RegisterStatusNotifierItem",
            |m| m.append1(me),
        );
    }

    fn rebuild(&mut self, d: &MenuData) {
        let mut items = vec![Item { id: 1, label: "Open ZTile".into(), kind: Kind::Normal, action: Some(TrayActionKey::Show) }];
        items.push(Item { id: 2, label: String::new(), kind: Kind::Separator, action: None });
        if d.layouts.is_empty() {
            items.push(Item { id: 3, label: "(no saved layouts)".into(), kind: Kind::Disabled, action: None });
        }
        for (i, (name, open)) in d.layouts.iter().enumerate() {
            // "_" is the mnemonic marker in dbusmenu labels
            items.push(Item {
                id: 100 + i as i32,
                label: name.replace('_', "__"),
                kind: Kind::Check(*open),
                action: Some(TrayActionKey::Open(name.clone())),
            });
        }
        items.push(Item { id: 4, label: String::new(), kind: Kind::Separator, action: None });
        items.push(Item { id: 5, label: "Start on login".into(), kind: Kind::Check(d.autostart), action: Some(TrayActionKey::Autostart) });
        items.push(Item { id: 6, label: "Quit".into(), kind: Kind::Normal, action: Some(TrayActionKey::Quit) });
        self.items = items;
        self.revision += 1;
    }

    fn props(it: &Item) -> PropMap {
        let mut p = PropMap::new();
        match &it.kind {
            Kind::Separator => {
                p.insert("type".into(), var("separator".to_string()));
            }
            Kind::Disabled => {
                p.insert("label".into(), var(it.label.clone()));
                p.insert("enabled".into(), var(false));
            }
            Kind::Normal => {
                p.insert("label".into(), var(it.label.clone()));
                if it.id == 6 {
                    p.insert("icon-name".into(), var("application-exit".to_string()));
                }
            }
            Kind::Check(on) => {
                p.insert("label".into(), var(it.label.clone()));
                p.insert("toggle-type".into(), var("checkmark".to_string()));
                p.insert("toggle-state".into(), var(*on as i32));
            }
        }
        p
    }

    fn layout(&self) -> (i32, PropMap, Vec<Variant<Box<dyn RefArg>>>) {
        let mut root = PropMap::new();
        root.insert("children-display".into(), var("submenu".to_string()));
        let children = self
            .items
            .iter()
            .map(|it| var((it.id, Self::props(it), Vec::<Variant<Box<dyn RefArg>>>::new())))
            .collect();
        (0, root, children)
    }

    fn item_props(&self) -> PropMap {
        let mut p = PropMap::new();
        p.insert("Category".into(), var("ApplicationStatus".to_string()));
        p.insert("Id".into(), var("ztile".to_string()));
        p.insert("Title".into(), var("ZTile".to_string()));
        p.insert("Status".into(), var("Active".to_string()));
        p.insert("IconName".into(), var(String::new()));
        p.insert("IconPixmap".into(), var(self.pixmaps.clone()));
        p.insert("ToolTip".into(), var((String::new(), self.pixmaps.clone(), "ZTile".to_string(), String::new())));
        p.insert("ItemIsMenu".into(), var(false));
        p.insert("Menu".into(), var(dbus::Path::from(MENU_PATH)));
        p
    }

    fn menu_props() -> PropMap {
        let mut p = PropMap::new();
        p.insert("Version".into(), var(3u32));
        p.insert("TextDirection".into(), var("ltr".to_string()));
        p.insert("Status".into(), var("normal".to_string()));
        p.insert("IconThemePath".into(), var(Vec::<String>::new()));
        p
    }

    /// Handle a call to one of the tray's objects. None = not ours.
    pub fn handle(&mut self, bus: &Bus, m: &Message, data: &MenuData) -> Option<Option<TrayAction>> {
        let path = m.path()?.to_string();
        if path != ITEM_PATH && path != MENU_PATH {
            return None;
        }
        let iface = m.interface().map(|i| i.to_string()).unwrap_or_default();
        let member = m.member().map(|x| x.to_string()).unwrap_or_default();
        let mut action = None;
        let reply = match (iface.as_str(), member.as_str()) {
            (PROPS, "GetAll") => {
                let p = if path == ITEM_PATH { self.item_props() } else { Self::menu_props() };
                m.method_return().append1(p)
            }
            (PROPS, "Get") => {
                let Ok((_, name)) = m.read2::<&str, &str>() else { return Some(None) };
                let mut p = if path == ITEM_PATH { self.item_props() } else { Self::menu_props() };
                match p.remove(name) {
                    Some(v) => m.method_return().append1(v),
                    None => {
                        bus.reply_error(m, "org.freedesktop.DBus.Error.UnknownProperty", "no such property");
                        return Some(None);
                    }
                }
            }
            (ITEM_IFACE, "Activate") | (ITEM_IFACE, "SecondaryActivate") => {
                action = Some(TrayAction::Show);
                m.method_return()
            }
            (ITEM_IFACE, _) => m.method_return(),
            (MENU_IFACE, "AboutToShow") => {
                self.rebuild(data);
                m.method_return().append1(true)
            }
            (MENU_IFACE, "AboutToShowGroup") => {
                self.rebuild(data);
                m.method_return().append2(vec![0i32], Vec::<i32>::new())
            }
            (MENU_IFACE, "GetLayout") => {
                if self.items.is_empty() {
                    self.rebuild(data);
                }
                m.method_return().append2(self.revision, self.layout())
            }
            (MENU_IFACE, "GetGroupProperties") => {
                let ids: Vec<i32> = m.read1::<Vec<i32>>().unwrap_or_default();
                let out: Vec<(i32, PropMap)> =
                    self.items.iter().filter(|it| ids.is_empty() || ids.contains(&it.id)).map(|it| (it.id, Self::props(it))).collect();
                m.method_return().append1(out)
            }
            (MENU_IFACE, "GetProperty") => {
                let Ok((id, name)) = m.read2::<i32, &str>() else { return Some(None) };
                let v = self.items.iter().find(|it| it.id == id).and_then(|it| Self::props(it).remove(name));
                match v {
                    Some(v) => m.method_return().append1(v),
                    None => m.method_return().append1(var(String::new())),
                }
            }
            (MENU_IFACE, "Event") => {
                if let Ok((id, kind)) = m.read2::<i32, &str>() {
                    if kind == "clicked" {
                        action = self.action_for(id);
                    }
                }
                m.method_return()
            }
            (MENU_IFACE, "EventGroup") => {
                if let Ok(evs) = m.read1::<Vec<(i32, String, Variant<Box<dyn RefArg>>, u32)>>() {
                    for (id, kind, _, _) in evs {
                        if kind == "clicked" {
                            action = self.action_for(id);
                        }
                    }
                }
                m.method_return().append1(Vec::<i32>::new())
            }
            _ => {
                bus.reply_error(m, "org.freedesktop.DBus.Error.UnknownMethod", "unknown method");
                return Some(None);
            }
        };
        bus.send(reply);
        Some(action)
    }

    fn action_for(&self, id: i32) -> Option<TrayAction> {
        let it = self.items.iter().find(|it| it.id == id)?;
        Some(match it.action.clone()? {
            TrayActionKey::Show => TrayAction::Show,
            TrayActionKey::Open(n) => TrayAction::OpenLayout(n),
            TrayActionKey::Autostart => TrayAction::ToggleAutostart,
            TrayActionKey::Quit => TrayAction::Quit,
        })
    }

    /// Tell the tray the menu changed (it re-asks with GetLayout).
    pub fn changed(&mut self, bus: &Bus, d: &MenuData) {
        self.rebuild(d);
        if let Ok(s) = Message::new_signal(MENU_PATH, MENU_IFACE, "LayoutUpdated") {
            bus.send(s.append2(self.revision, 0i32));
        }
    }
}
