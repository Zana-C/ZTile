//! Saved layouts: %APPDATA%\ZTile\layouts.json

use crate::apps::AppSpec;
use crate::json::Json;
use crate::layout::Node;
use crate::win::Monitor;
use std::path::PathBuf;

const VERSION: f64 = 1.0;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LAYOUTS: usize = 200;

/// Enough to find "the same" monitor again after reboots/docking.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MonitorId {
    /// GDI device name, e.g. \\.\DISPLAY1
    pub device: String,
    pub width: i32,
    pub height: i32,
}

impl MonitorId {
    pub fn of(m: &Monitor) -> MonitorId {
        MonitorId { device: m.device.clone(), width: m.rect.right - m.rect.left, height: m.rect.bottom - m.rect.top }
    }

    /// Same device name with same size > same size > same device > primary.
    pub fn find(&self, mons: &[Monitor]) -> Option<Monitor> {
        let size = |m: &Monitor| m.rect.right - m.rect.left == self.width && m.rect.bottom - m.rect.top == self.height;
        mons.iter()
            .find(|m| m.device == self.device && size(m))
            .or_else(|| mons.iter().find(|m| size(m)))
            .or_else(|| mons.iter().find(|m| m.device == self.device))
            .or_else(|| mons.iter().find(|m| m.primary))
            .or(mons.first())
            .cloned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedLayout {
    pub name: String,
    pub monitor: MonitorId,
    pub tree: Node,
    pub gap: i32,
    pub borderless: bool,
    /// one entry per cell, in cell order; None = empty cell ("+")
    pub apps: Vec<Option<AppSpec>>,
}

impl SavedLayout {
    fn to_json(&self) -> Json {
        Json::obj(vec![
            ("name", Json::str(&self.name)),
            (
                "monitor",
                Json::obj(vec![
                    ("device", Json::str(&self.monitor.device)),
                    ("width", Json::Num(self.monitor.width as f64)),
                    ("height", Json::Num(self.monitor.height as f64)),
                ]),
            ),
            ("tree", self.tree.to_json()),
            ("gap", Json::Num(self.gap as f64)),
            ("borderless", Json::Bool(self.borderless)),
            ("apps", Json::Arr(self.apps.iter().map(|a| a.as_ref().map_or(Json::Null, AppSpec::to_json)).collect())),
        ])
    }

    fn from_json(j: &Json) -> Option<SavedLayout> {
        let tree = Node::from_json(j.get("tree")?)?;
        let m = j.get("monitor");
        let num = |k: &str| m.and_then(|m| m.get(k)).and_then(Json::as_f64).unwrap_or(0.0) as i32;
        let mut apps: Vec<Option<AppSpec>> = j
            .get("apps")
            .and_then(Json::as_arr)
            .map(|a| a.iter().map(AppSpec::from_json).collect())
            .unwrap_or_default();
        apps.resize(tree.cell_count(), None);
        Some(SavedLayout {
            name: j.get("name")?.as_str()?.to_string(),
            monitor: MonitorId {
                device: m.map(|m| m.str_or_empty("device")).unwrap_or_default(),
                width: num("width"),
                height: num("height"),
            },
            tree,
            gap: j.get("gap").and_then(Json::as_f64).filter(|g| g.is_finite()).unwrap_or(0.0).clamp(0.0, 64.0) as i32,
            borderless: j.get("borderless").and_then(Json::as_bool).unwrap_or(false),
            apps,
        })
    }
}

#[derive(Debug, Default)]
pub struct Store {
    pub layouts: Vec<SavedLayout>,
}

impl Store {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("ZTile").join("layouts.json")
    }

    /// Missing file = empty store. A corrupt file is kept aside as .bad, never overwritten silently.
    pub fn load() -> Store {
        let path = Self::path();
        let Ok(meta) = std::fs::metadata(&path) else { return Store::default() };
        // A real file is a few KB; anything huge is broken or tampered with.
        let res = if meta.len() > MAX_FILE_BYTES {
            Err(format!("file larger than {} KB", MAX_FILE_BYTES / 1024))
        } else {
            std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Self::from_text(&t))
        };
        match res {
            Ok(s) => s,
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let _ = std::fs::rename(&path, &bad);
                eprintln!("  ! couldn't read {} ({e}); moved it aside as {}.", path.display(), bad.display());
                Store::default()
            }
        }
    }

    fn from_text(text: &str) -> Result<Store, String> {
        let j = Json::parse(text.trim_start_matches('\u{feff}'))?;
        let layouts = j
            .get("layouts")
            .and_then(Json::as_arr)
            .ok_or("missing \"layouts\"")?
            .iter()
            .filter_map(SavedLayout::from_json)
            .take(MAX_LAYOUTS)
            .collect();
        Ok(Store { layouts })
    }

    fn to_text(&self) -> String {
        Json::obj(vec![
            ("version", Json::Num(VERSION)),
            ("layouts", Json::Arr(self.layouts.iter().map(SavedLayout::to_json).collect())),
        ])
        .pretty()
    }

    /// Write to a temp file, then rename over the old one: never leaves a half-written file.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, &path)
    }

    pub fn get(&self, name: &str) -> Option<&SavedLayout> {
        self.layouts.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// Replace a layout with the same name (case-insensitive) or append.
    pub fn put(&mut self, l: SavedLayout) {
        match self.layouts.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&l.name)) {
            Some(x) => *x = l,
            None => self.layouts.push(l),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let n = self.layouts.len();
        self.layouts.retain(|l| !l.name.eq_ignore_ascii_case(name));
        self.layouts.len() != n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    #[test]
    fn roundtrip() {
        let mut s = Store::default();
        s.put(SavedLayout {
            name: "Social".into(),
            monitor: MonitorId { device: "\\\\.\\DISPLAY1".into(), width: 1920, height: 1080 },
            tree: layout::preset("2x2").unwrap(),
            gap: 4,
            borderless: false,
            apps: vec![
                Some(AppSpec {
                    exe: "C:\\Apps\\browser.exe".into(),
                    aumid: "Browser._crx_x".into(),
                    class: "Chrome_WidgetWin_1".into(),
                    name: "Chat \"Web\"".into(),
                    window: String::new(),
                }),
                None,
                None,
                None,
            ],
        });
        let back = Store::from_text(&s.to_text()).unwrap();
        assert_eq!(back.layouts, s.layouts);
    }

    #[test]
    fn put_replaces_by_name() {
        let mk = |name: &str, gap| SavedLayout {
            name: name.into(),
            monitor: MonitorId::default(),
            tree: layout::preset("full").unwrap(),
            gap,
            borderless: false,
            apps: vec![None],
        };
        let mut s = Store::default();
        s.put(mk("Work", 0));
        s.put(mk("Games", 0));
        s.put(mk("Games", 8));
        assert_eq!(s.layouts.len(), 2);
        assert_eq!(s.get("games").unwrap().gap, 8);
        assert!(s.remove("GAMES"));
        assert!(s.get("Games").is_none());
    }
}
