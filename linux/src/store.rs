//! Saved layouts: $XDG_CONFIG_HOME/ztile/layouts.json (usually ~/.config/ztile).
//! Same file format as the Windows version; app entries differ per platform.

use crate::files;
use std::path::PathBuf;
use ztile_core::json::Json;
use ztile_core::layout::Node;

const VERSION: f64 = 1.0;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LAYOUTS: usize = 200;
const MAX_STR: usize = 256;

/// What a cell remembers about its app. No window titles by default: they change
/// all the time and often contain private text (chat names...).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AppSpec {
    /// app id = .desktop file name, e.g. "discord", "brave-<id>-Default"
    pub app: String,
    /// display name, e.g. "Discord"
    pub name: String,
    /// "" = any window of the app; else a title fragment the user picked
    pub window: String,
}

fn bounded(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl AppSpec {
    pub fn to_json(&self) -> Json {
        let mut v = vec![("app", Json::str(&self.app)), ("name", Json::str(&self.name))];
        if !self.window.is_empty() {
            v.push(("window", Json::str(&self.window)));
        }
        Json::obj(v)
    }

    pub fn from_json(j: &Json) -> Option<AppSpec> {
        let app = j.get("app")?.as_str()?;
        if app.is_empty() {
            return None; // also: entries written by the Windows version (no "app")
        }
        Some(AppSpec {
            app: bounded(app, MAX_STR),
            name: bounded(&j.str_or_empty("name"), MAX_STR),
            window: bounded(&j.str_or_empty("window"), MAX_STR),
        })
    }

    /// {app, window} as the KWin script wants it
    pub fn engine_json(&self) -> Json {
        Json::obj(vec![("app", Json::str(&self.app)), ("window", Json::str(&self.window))])
    }
}

/// Enough to find the same display again: output name + physical size.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MonitorId {
    pub name: String,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedLayout {
    pub name: String,
    pub monitor: MonitorId,
    pub tree: Node,
    pub gap: i32,
    pub borderless: bool,
    /// false (default): the layout only changes in the editor
    pub keep_changes: bool,
    /// one per cell, None = empty
    pub apps: Vec<Option<AppSpec>>,
}

impl SavedLayout {
    fn to_json(&self) -> Json {
        Json::obj(vec![
            ("name", Json::str(&self.name)),
            (
                "monitor",
                Json::obj(vec![
                    ("name", Json::str(&self.monitor.name)),
                    ("width", Json::Num(self.monitor.width as f64)),
                    ("height", Json::Num(self.monitor.height as f64)),
                ]),
            ),
            ("tree", self.tree.to_json()),
            ("gap", Json::Num(self.gap as f64)),
            ("borderless", Json::Bool(self.borderless)),
            ("keep_changes", Json::Bool(self.keep_changes)),
            ("apps", Json::Arr(self.apps.iter().map(|a| a.as_ref().map_or(Json::Null, AppSpec::to_json)).collect())),
        ])
    }

    fn from_json(j: &Json) -> Option<SavedLayout> {
        let name = bounded(j.get("name")?.as_str()?.trim(), 60);
        if name.is_empty() {
            return None;
        }
        let tree = Node::from_json(j.get("tree")?)?;
        let m = j.get("monitor");
        let num = |k: &str| {
            m.and_then(|m| m.get(k)).and_then(Json::as_f64).filter(|v| v.is_finite() && (0.0..100000.0).contains(v)).unwrap_or(0.0)
                as i32
        };
        let mname = m.map(|m| {
            let n = m.str_or_empty("name");
            if n.is_empty() { m.str_or_empty("device") } else { n }
        });
        let n = tree.cell_count();
        let mut apps: Vec<Option<AppSpec>> =
            j.get("apps").and_then(Json::as_arr).map(|a| a.iter().take(n).map(AppSpec::from_json).collect()).unwrap_or_default();
        apps.resize(n, None);
        Some(SavedLayout {
            name,
            monitor: MonitorId { name: bounded(&mname.unwrap_or_default(), 64), width: num("width"), height: num("height") },
            tree,
            gap: j.get("gap").and_then(Json::as_f64).filter(|g| g.is_finite()).unwrap_or(0.0).clamp(0.0, 64.0) as i32,
            borderless: j.get("borderless").and_then(Json::as_bool).unwrap_or(false),
            keep_changes: j.get("keep_changes").and_then(Json::as_bool).unwrap_or(false),
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
        files::config_home().join("ztile").join("layouts.json")
    }

    /// Missing file = empty store. A file that can be read but not parsed is kept
    /// aside as .bad; one that can't be read right now is left alone.
    pub fn load() -> Store {
        let path = Self::path();
        let Ok(meta) = std::fs::metadata(&path) else { return Store::default() };
        if meta.len() > MAX_FILE_BYTES {
            return Self::bad(&path, "file larger than 1 MB");
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("ztile: couldn't read {}: {e}", path.display());
                return Store::default();
            }
        };
        match Self::from_text(&text) {
            Ok(s) => s,
            Err(e) => Self::bad(&path, &e),
        }
    }

    fn bad(path: &std::path::Path, why: &str) -> Store {
        let bad = path.with_extension("json.bad");
        let _ = std::fs::rename(path, &bad);
        eprintln!("ztile: couldn't read {} ({why}); moved it aside as {}.", path.display(), bad.display());
        Store::default()
    }

    pub fn from_text(text: &str) -> Result<Store, String> {
        let j = Json::parse(text.trim_start_matches('\u{feff}'))?;
        let mut layouts: Vec<SavedLayout> = Vec::new();
        for l in j.get("layouts").and_then(Json::as_arr).ok_or("missing \"layouts\"")?.iter().take(MAX_LAYOUTS) {
            if let Some(l) = SavedLayout::from_json(l) {
                if layouts.iter().all(|x| !x.name.to_lowercase().eq(&l.name.to_lowercase())) {
                    layouts.push(l);
                }
            }
        }
        Ok(Store { layouts })
    }

    pub fn to_text(&self) -> String {
        Json::obj(vec![
            ("version", Json::Num(VERSION)),
            ("layouts", Json::Arr(self.layouts.iter().map(SavedLayout::to_json).collect())),
        ])
        .pretty()
    }

    /// Readable by the user only: a pinned cell keeps a window title fragment.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        files::private_dir(path.parent().unwrap())?;
        files::write_atomic(&path, self.to_text().as_bytes(), 0o600)
    }

    pub fn get(&self, name: &str) -> Option<&SavedLayout> {
        let n = name.to_lowercase();
        self.layouts.iter().find(|l| l.name.to_lowercase() == n)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut SavedLayout> {
        let n = name.to_lowercase();
        self.layouts.iter_mut().find(|l| l.name.to_lowercase() == n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztile_core::layout;

    #[test]
    fn roundtrip_and_python_compat() {
        // written by the Python version / the UI
        let text = r#"{"version": 1, "layouts": [{"name": "Chat", "monitor": {"name": "eDP-1", "width": 2560, "height": 1600},
            "tree": {"dir": "cols", "sizes": [1, 1], "children": ["cell", "cell"]}, "gap": 4, "borderless": false,
            "keep_changes": true, "apps": [{"app": "discord", "name": "Discord"}, {"app": "x", "name": "X", "window": "Chat"}]}]}"#;
        let s = Store::from_text(text).unwrap();
        let l = &s.layouts[0];
        assert_eq!((l.monitor.width, l.gap, l.keep_changes), (2560, 4, true));
        assert_eq!(l.apps[1].as_ref().unwrap().window, "Chat");
        assert_eq!(Store::from_text(&s.to_text()).unwrap().layouts, s.layouts);
    }

    #[test]
    fn untrusted_values_are_bounded() {
        let long = "x".repeat(5000);
        let text = format!(
            r#"{{"layouts": [{{"name": "{long}", "tree": "cell", "gap": 1e308, "apps": [{{"app": "{long}", "window": 5}}]}},
                {{"name": "Bad", "tree": {{"dir": "evil"}}}}, {{"name": "Win", "tree": "cell", "apps": [{{"exe": "C:\\x.exe"}}]}}]}}"#
        );
        let s = Store::from_text(&text).unwrap();
        assert_eq!(s.layouts.len(), 2);
        assert_eq!(s.layouts[0].name.len(), 60);
        assert_eq!(s.layouts[0].gap, 64);
        assert_eq!(s.layouts[0].apps[0].as_ref().unwrap().app.len(), MAX_STR);
        assert_eq!(s.layouts[0].apps[0].as_ref().unwrap().window, "");
        assert_eq!(s.layouts[1].apps, vec![None]); // Windows entries don't apply here
        let _ = layout::preset("full");
    }
}
