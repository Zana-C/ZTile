//! Writing files safely, and escaping values for .desktop files.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if Path::new(&v).is_absolute() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn cache_home() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// Write via a fresh temp file in the same folder, then rename over the target:
/// never half-written, and a symlink planted at the target is replaced, not followed.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?; // umask may have cut bits
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// A folder only the user can read (layouts can contain window titles).
pub fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// A string value for a .desktop file: no control characters (a newline would
/// start a new key, e.g. a forged `Exec=`), backslashes escaped.
pub fn desktop_value(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .replace('\\', "\\\\")
}

/// One argument of a .desktop `Exec=` line, quoted per the Desktop Entry spec.
pub fn exec_arg(arg: &str) -> String {
    let safe = !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "_./+@%:,=-".contains(c));
    if safe {
        return arg.to_string();
    }
    let mut inner = String::new();
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            inner.push('\\');
        }
        if !c.is_control() {
            inner.push(c);
        }
    }
    // the whole line is also a string value: its backslashes are escaped once more
    format!("\"{}\"", inner.replace('\\', "\\\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_values_cannot_add_keys() {
        let v = desktop_value("Work\nExec=rm -rf ~\r\0");
        assert!(!v.contains('\n') && !v.contains('\r') && !v.contains('\0'));
        assert_eq!(desktop_value("a\\b"), "a\\\\b");
    }

    #[test]
    fn exec_args_are_quoted() {
        assert_eq!(exec_arg("/usr/bin/ztile"), "/usr/bin/ztile");
        assert_eq!(exec_arg("/home/a b/ztile"), "\"/home/a b/ztile\"");
        let q = exec_arg("/x/$HOME/`id`");
        assert!(q.starts_with('"') && q.contains("\\\\$HOME") && q.contains("\\\\`id\\\\`"));
    }

    #[test]
    fn atomic_write_replaces_symlink_not_target() {
        let dir = std::env::temp_dir().join(format!("ztile-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim");
        fs::write(&victim, "keep").unwrap();
        let link = dir.join("out");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        write_atomic(&link, b"new", 0o600).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
        assert!(!fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::metadata(&link).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
