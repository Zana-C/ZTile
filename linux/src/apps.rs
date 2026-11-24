//! Installed apps (.desktop files): names, icons and starting them.
//!
//! A window's identity on Plasma is its app id (Wayland app_id or X11 class), which
//! normally is the name of its .desktop file: "org.kde.dolphin", "discord", and
//! browser web apps get their own ("brave-<id>-Default"). A cell stores that id.
//!
//! Nothing here is kept in memory for long: entries are read when needed.

use crate::files;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const MAX_ENTRY_BYTES: u64 = 256 * 1024;
const MAX_ICON_BYTES: u64 = 4 * 1024 * 1024;

pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub icon: String,
    pub wm_class: String,
}

/// $XDG_DATA_HOME, then $XDG_DATA_DIRS (user entries override system ones).
pub fn data_dirs() -> Vec<PathBuf> {
    let mut out = vec![files::data_home()];
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let dirs = if dirs.is_empty() { "/usr/local/share:/usr/share".to_string() } else { dirs };
    for d in dirs.split(':').filter(|d| d.starts_with('/')) {
        let p = PathBuf::from(d);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn lang() -> Vec<String> {
    let l = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default();
    let l = l.split(['.', '@']).next().unwrap_or("").to_string();
    let short = l.split('_').next().unwrap_or("").to_string();
    [l, short].into_iter().filter(|s| !s.is_empty()).collect()
}

fn parse(path: &Path, id: &str) -> Option<Entry> {
    if std::fs::metadata(path).ok()?.len() > MAX_ENTRY_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let langs = lang();
    let (mut name, mut lname, mut icon, mut wm, mut ty) = (String::new(), None::<(usize, String)>, String::new(), String::new(), String::new());
    let mut hidden = false;
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if !in_entry {
            continue;
        }
        let (k, v) = (k.trim(), v.trim().to_string());
        match k {
            "Name" if name.is_empty() => name = v,
            "Icon" if icon.is_empty() => icon = v,
            "StartupWMClass" if wm.is_empty() => wm = v,
            "Type" => ty = v,
            "Hidden" => hidden = v == "true",
            _ => {
                if let Some(l) = k.strip_prefix("Name[").and_then(|r| r.strip_suffix(']')) {
                    if let Some(rank) = langs.iter().position(|x| x == l) {
                        if lname.as_ref().is_none_or(|(r, _)| rank < *r) {
                            lname = Some((rank, v));
                        }
                    }
                }
            }
        }
    }
    if ty != "Application" || hidden {
        return None;
    }
    Some(Entry {

        path: path.to_path_buf(),
        name: lname.map(|(_, n)| n).unwrap_or(if name.is_empty() { id.to_string() } else { name }),
        icon,
        wm_class: wm,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 255 && !id.starts_with(['.', '-']) && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The entry for an app id: exact .desktop file name, or (scanning) an entry whose
/// StartupWMClass is that id. Case-insensitive like window classes.
pub fn find(app_id: &str) -> Option<Entry> {
    if !valid_id(app_id) {
        return None;
    }
    let dirs: Vec<PathBuf> = data_dirs().into_iter().map(|d| d.join("applications")).collect();
    for d in &dirs {
        let p = d.join(format!("{app_id}.desktop"));
        if p.is_file() {
            return parse(&p, app_id);
        }
    }
    let lower = app_id.to_lowercase();
    for d in &dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(id) = fname.strip_suffix(".desktop") else { continue };
            if id.to_lowercase() == lower {
                if let Some(x) = parse(&e.path(), id) {
                    return Some(x);
                }
            }
        }
    }
    for d in &dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(id) = fname.strip_suffix(".desktop") else { continue };
            if let Some(x) = parse(&e.path(), id) {
                if x.wm_class.to_lowercase() == lower {
                    return Some(x);
                }
            }
        }
    }
    None
}

pub fn display_name(app_id: &str) -> String {
    find(app_id).map(|e| e.name).unwrap_or_else(|| app_id.to_string())
}

/// Start an app from its .desktop entry. Only installed entries are started, never
/// a command line taken from a file. `kioclient exec` starts it the way the Plasma
/// menu does, in its own systemd unit: started by us directly it would live in our
/// cgroup (its memory shown as ours, and killed when we quit).
pub fn launch(app_id: &str) -> Option<Child> {
    let e = find(app_id)?;
    if !e.path.is_absolute() {
        return None;
    }
    for (prog, args) in [("kioclient", ["exec"]), ("gio", ["launch"])] {
        let c = Command::new(prog)
            .args(args)
            .arg(&e.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(c) = c {
            return Some(c);
        }
    }
    None
}

// ---------------------------------------------------------------- icons

/// Straight-alpha RGBA pixels.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

fn icon_file(icon: &str) -> Option<PathBuf> {
    if icon.starts_with('/') {
        return Path::new(icon).is_file().then(|| PathBuf::from(icon));
    }
    if !valid_id(icon) {
        return None;
    }
    let mut roots: Vec<PathBuf> = vec![files::home().join(".icons")];
    for d in data_dirs() {
        roots.push(d.join("icons"));
    }
    let themes = ["hicolor", "breeze", "breeze-dark"];
    let pngs = ["256x256", "128x128", "96x96", "64x64", "48x48"];
    for root in &roots {
        for t in themes {
            for s in pngs {
                let p = root.join(t).join(s).join("apps").join(format!("{icon}.png"));
                if p.is_file() {
                    return Some(p);
                }
            }
            for sub in ["scalable/apps", "apps/48", "apps/64", "apps/32"] {
                let p = root.join(t).join(sub).join(format!("{icon}.svg"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    for d in data_dirs() {
        let p = d.join("pixmaps").join(format!("{icon}.png"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// An app's icon at about `size` px, or None.
pub fn icon(app_id: &str, size: u32) -> Option<Image> {
    let e = find(app_id)?;
    let path = icon_file(if e.icon.is_empty() { app_id } else { &e.icon })?;
    if std::fs::metadata(&path).ok()?.len() > MAX_ICON_BYTES {
        return None;
    }
    let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "png" { decode_png(&path) } else { render_svg(&path, size) }
}

fn decode_png(path: &Path) -> Option<Image> {
    let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    d.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut r = d.read_info().ok()?;
    let (w, h) = (r.info().width, r.info().height);
    if w == 0 || h == 0 || w > 1024 || h > 1024 {
        return None;
    }
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let px = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        _ => return None,
    };
    Some(Image { w, h, px })
}

fn render_svg(path: &Path, size: u32) -> Option<Image> {
    let data = std::fs::read(path).ok()?;
    if data.starts_with(&[0x1f, 0x8b]) {
        return None; // .svgz: would need a decompressor; apps rarely ship these
    }
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let ts = tree.size();
    let scale = size as f32 / ts.width().max(ts.height()).max(1.0);
    let mut pm = resvg::tiny_skia::Pixmap::new(size, size)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    // tiny-skia is premultiplied; we use straight alpha
    let px = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Some(Image { w: size, h: size, px })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_validated() {
        assert!(valid_id("org.kde.dolphin"));
        assert!(valid_id("brave-hnpfjngllnobngcgfapefoaidbinmjnm-Default"));
        assert!(!valid_id("../../etc/passwd"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("-option"));
        assert!(!valid_id(""));
        assert!(find("../../../etc/passwd").is_none());
    }
}
