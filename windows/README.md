# ZTile for Windows

The full documentation (features, installing, usage, data, security) is in the
[main README](../README.md). This file is a short reference for this folder.

## Build

Requires Rust (MSVC toolchain) and the Visual Studio Build Tools
("Desktop development with C++").

```powershell
cargo test
cargo build --release   # target\release\ztile.exe, a single portable file
```

The release profile optimizes for size and enables LTO. `.cargo/config.toml`
adds the hardening linker flags (System32-only DLL loading, Control Flow Guard,
CET). The only external dependency is Microsoft's `windows` crate; the layout
model and JSON code live in `../core` and are shared with the Linux version.

## Source files

| File | What it does |
|---|---|
| `src/main.rs` | Entry point, process hardening, single instance, command line |
| `src/workspace.rs` | Live groups: tiling, taskbar button, drag & drop, launching apps |
| `src/ui.rs` | Main window and layout editor (GDI) |
| `src/tray.rs` | Tray icon, its menu, start with Windows |
| `src/layout.rs` | The shared layout model (`../core`) with Win32 rectangles |
| `src/apps.rs` | Identifying apps, matching windows, starting programs |
| `src/store.rs` | Saving and loading layouts |
| `src/win.rs` | Window helpers (enumeration, placement, DPI, monitors) |
| `src/icon.rs` | App icons and the group's composite taskbar icon |
| `src/json.rs` | The shared JSON reader/writer (`../core`) |

`PLAN.md` holds the design notes and decisions (in Turkish).
