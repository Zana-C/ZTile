# ZTile

A tiny, fast window layout manager for Windows. Arrange apps into a grid, save
the arrangement, and bring the whole group back with one click. Each group gets
**one taskbar button** instead of one per app.

- ~430 KB executable, ~2 MB of memory, no CPU use while idle
- Plain Win32 in Rust: no runtime, no framework, no network access

## Features

- **Layouts**: presets (halves, thirds, 2×2, one large + two/three small…) or your
  own: split any cell side by side or top and bottom, drag the dividers to resize.
- **Saved layouts**: name a layout, assign apps to its cells, open it from the
  ZTile window or the tray menu. Apps that aren't running are started and put in
  place automatically.
- **One taskbar button per group**, showing a small preview of the grid with the
  app icons. Click it to bring the group up; click again to minimize it.
- **Several groups at once**, e.g. a chat group on the laptop screen and a work
  group on the monitor. Groups on the same display stack: the one you click comes
  to the front, minimizing it reveals the one below.
- **No focus stealing**: ZTile never activates the apps it arranges, so chat apps
  don't mark messages as read just because their group came to the front.
- **Drag and drop** with live feedback:
  - onto another cell (blue) – the two apps swap places, even across groups
  - back onto its own cell (grey) – it snaps back
  - outside the grid (red) – the app leaves the group
  - any window onto an empty cell – it joins the group
- **Empty cells** show a `+`: put an open window there, start a program, or
  restart the app that used to live in that cell.
- **Any window or one specific window**: a cell normally takes any window of its
  app. You can also pin it to one specific window of an app (for example a
  pop-out window) from the cell menu in the editor.
- Square corners inside the grid (Windows 11), optional hidden title bars for
  apps with a classic title bar, adjustable gap between cells.
- Follows the system light/dark theme. Optional start with Windows.

## Requirements

- Windows 10 (1703 or later) or Windows 11, 64-bit
- To build: [Rust](https://www.rust-lang.org/tools/install) (MSVC toolchain) and
  the Visual Studio Build Tools with the "Desktop development with C++" workload

## Getting started

### Build from source

```powershell
git clone https://github.com/Zana-C/ZTile.git
cd ZTile
cargo build --release
```

The program is `target\release\ztile.exe`. It's a single file; copy it anywhere
you like.

> **Windows may warn about the file.** ZTile isn't code-signed yet, so Windows
> doesn't recognize it. SmartScreen shows "Windows protected your PC": choose
> *More info → Run anyway*. If **Smart App Control** is turned on, Windows blocks
> unsigned programs you build yourself without that option; you'd have to turn
> Smart App Control off (Windows Security → App & browser control).

### First use

1. Run `ztile.exe`. The ZTile window opens and an icon appears in the system tray
   (Windows may tuck it under the `^` arrow).
2. Click **New layout**, give it a name, pick a display and a preset.
3. Click each cell to choose an app: one that's open right now, or a program /
   Start menu shortcut. Use the cell menu to split or remove cells; drag the
   lines between cells to resize them.
4. Click **Save and open**. The apps move into place (or start, if they weren't
   running), and the group gets its own taskbar button.

From then on, open a layout by clicking its card in the ZTile window, or from
the tray icon's menu. Right-click a card to edit, close or delete it. Closing the
ZTile window keeps it running in the tray; use **Quit** in the tray menu to exit.

A saved layout only changes when you edit it. Rearranging an open group
(swapping apps by dragging, filling an empty cell) affects that group only; the
next time you open the layout it's back to how you saved it. If you'd rather
keep such changes, turn on **Save changes made in the open group** in the
layout's editor.

## Your data

Layouts are stored in `%APPDATA%\ZTile\layouts.json`, on your PC only. For each
cell ZTile keeps:

- the path of the app's executable and its Windows app ID (AUMID), so it can find
  and start the app again
- the app's name (e.g. "Discord"), for display
- a window title fragment **only** if you pinned the cell to one specific window

ZTile has no network code, sends nothing anywhere and collects no telemetry.
The "Start with Windows" option adds a value named `ZTile` under
`HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.

## Security

- The only dependency is Microsoft's official [`windows`](https://github.com/microsoft/windows-rs)
  crate (plus the macro crates it uses at build time). Small things like JSON
  parsing are written in-house.
- No code is injected into other programs; ZTile observes windows through
  standard out-of-process accessibility events.
- Only local `.exe` files or valid app IDs are ever started from a saved layout;
  scripts, documents and network paths are refused. Input read from disk is size-
  and depth-limited.
- The executable is built with ASLR, DEP, Control Flow Guard and CET shadow-stack
  compatibility, loads its DLLs from System32 only, and disables legacy DLL
  injection points at startup.

## Known limitations

- Windows of apps running as administrator can't be moved unless ZTile also runs
  as administrator (a Windows security boundary).
- "Hide title bars" only works for apps with a classic title bar. Apps that draw
  their own (most Chromium/Electron apps) keep theirs, since their buttons live there.
- Some apps enforce a minimum size and may not fit very small cells.
- If ZTile is force-killed (e.g. from Task Manager), the grouped apps' taskbar
  buttons stay hidden until Explorer restarts. Quitting normally restores them.
- A layout can have up to 16 cells.

## Command line (optional)

Everything is available from the window and the tray. For scripting:

```text
ztile                 open the ZTile window (starts ZTile if needed)
ztile --tray          start in the tray only
ztile open <NAME>     open a saved layout   (--mon N: on display N)
ztile layouts         list presets and saved layouts
ztile list            list displays and open windows
ztile quit            dissolve all groups and exit
```

Run `ztile help` for the full list. ZTile is a windowed app, so the shell doesn't
wait for it and output can appear after the prompt; pipe it to see it in order,
e.g. `ztile list | more`.

## Uninstall

Quit ZTile from the tray (turn off "Start with Windows" first if you enabled it),
then delete `ztile.exe` and the `%APPDATA%\ZTile` folder.

## Project layout

| File | What it does |
|---|---|
| `src/main.rs` | Entry point, single-instance handling, command line |
| `src/workspace.rs` | Live groups: tiling, taskbar button, drag & drop, launching apps |
| `src/ui.rs` | Main window and layout editor (GDI) |
| `src/tray.rs` | Tray icon, its menu, start with Windows |
| `src/layout.rs` | Layout model (split tree), presets, geometry |
| `src/apps.rs` | Identifying apps, matching windows, starting programs |
| `src/store.rs` | Saving and loading layouts |
| `src/win.rs` | Window helpers (enumeration, placement, DPI, monitors) |
| `src/icon.rs` | App icons and the group's composite taskbar icon |
| `src/json.rs` | Minimal JSON reader/writer |

## License

ZTile is released under the [MIT License](LICENSE). It includes code from
Microsoft's Windows crates for Rust, also under the MIT License; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). If you redistribute
`ztile.exe`, include both files.

Windows is a trademark of the Microsoft group of companies. Other app names
mentioned here belong to their respective owners; ZTile isn't affiliated with them.
