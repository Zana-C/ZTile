# ZTile

A tiny window layout manager for **Windows** and **KDE Plasma (Linux)**.
Arrange apps into a grid, save the arrangement, and bring the whole group back
with one click. Each group gets **one taskbar button** instead of one per app.

- **Windows 10/11:** a single ~430 KB `.exe` (Rust, plain Win32). About 2 MB of
  memory and no CPU use while idle.
- **KDE Plasma 6 (Wayland):** a ~1.7 MB program (Rust) plus a KWin script. About
  2 MB of memory and no CPU use while idle; the settings window is a separate
  process that only runs while it's open.

Both versions behave the same way, use the same layout file format, have no
network code and collect no telemetry.

---

## Contents

- [Features](#features)
- [Installation](#installation)
  - [Windows](#windows)
  - [Linux (KDE Plasma)](#linux-kde-plasma)
- [Using ZTile](#using-ztile)
- [Command line](#command-line)
- [Your data and privacy](#your-data-and-privacy)
- [Security](#security)
- [How it works](#how-it-works)
- [Known limitations](#known-limitations)
- [Troubleshooting](#troubleshooting)
- [Building and testing](#building-and-testing)
- [Repository layout](#repository-layout)
- [License](#license)

---

## Features

- **Layouts:** start from a preset (full screen, halves, thirds, 2×2, one large plus
  two or three small…) or build your own. Split any cell side by side or top and
  bottom, and drag the dividers to resize.
- **Saved layouts:** give a layout a name, assign an app to each cell, and open it
  from the ZTile window or the tray menu. Apps that aren't running are started
  and put in place automatically.
- **One taskbar button per group**, showing a small preview of the grid with the
  app icons. Click it to bring the group up; click it again to minimize it.
- **Several groups at once**, for example a chat group on the laptop screen and a
  work group on the monitor. Groups on the same display stack like windows: the
  one you click comes to the front, and minimizing it reveals the one below.
- **No focus stealing:** ZTile never activates the apps it arranges. Chat apps
  don't mark messages as read just because their group came to the front.
- **Drag and drop**, with live feedback while you drag:
  - onto another cell (blue): the two apps swap places, even across groups
  - back onto its own cell, or only a short distance (grey): it snaps back
  - outside the grid (red): the app leaves the group
  - any window onto an empty cell: it joins the group
- **Empty cells** show a `+`. Click it to put an open window there, start an app,
  or restart the app that used to live in that cell.
- **Any window or one specific window:** a cell normally takes any window of its
  app. From the cell menu in the editor you can pin it to one specific window
  instead, such as a pop-out window.
- **Layouts stay as you saved them.** Rearranging an open group affects only that
  group. To keep such changes, turn on *Save changes made in the open group* for
  that layout.
- Optional hidden title bars, an adjustable gap between cells, light/dark theme,
  optional start on login.

Platform extras:

| | Windows | KDE Plasma |
|---|---|---|
| Browser web apps (WhatsApp, Instagram… installed from Chrome/Edge/Brave) told apart | ✓ (AppUserModelID) | ✓ (app id) |
| Square corners inside the grid | ✓ (Windows 11) | n/a |
| Groups follow you to the current virtual desktop | n/a | ✓ |
| Re-tiles open groups when a display is added or removed | – | ✓ |
| Windows get their taskbar buttons back if ZTile is killed | – (until Explorer restarts) | ✓ |

---

## Installation

### Windows

**Requirements:** Windows 10 (1703 or later) or Windows 11, 64-bit.

ZTile is a single portable file. It doesn't need an installer: put `ztile.exe`
anywhere and run it. To build it yourself:

1. Install [Rust](https://www.rust-lang.org/tools/install) with the MSVC toolchain,
   and the Visual Studio Build Tools with the "Desktop development with C++" workload.
2. Build it:

   ```powershell
   git clone https://github.com/Zana-C/ZTile.git
   cd ZTile\windows
   cargo build --release
   ```

3. The program is `windows\target\release\ztile.exe`.

> **Windows may warn about the file.** ZTile isn't code-signed yet, so SmartScreen
> shows "Windows protected your PC". Choose *More info → Run anyway*. If
> **Smart App Control** is on, Windows blocks unsigned programs you build yourself
> with no way around it; you'd have to turn Smart App Control off
> (Windows Security → App & browser control).

**Uninstall:** quit ZTile from the tray (turn off "Start with Windows" first if
you enabled it), then delete `ztile.exe` and the `%APPDATA%\ZTile` folder.

### Linux (KDE Plasma)

**Requirements:**

- KDE Plasma 6 on Wayland (developed on Plasma 6.8). Other desktops and X11 sessions
  aren't supported: on Wayland only the compositor may move other apps' windows,
  so ZTile runs its window handling inside KWin.
- To build: [Rust](https://rustup.rs) (Arch/CachyOS: `sudo pacman -S rustup` then
  `rustup default stable`).
- For the settings window: Python 3.10+ with PyQt6 for the system Python. ZTile
  itself (groups, tray, command line) runs without it.

  | Distribution | Command |
  |---|---|
  | Arch, CachyOS, Manjaro | `sudo pacman -S python-pyqt6` |
  | Fedora | `sudo dnf install python3-pyqt6` |
  | Debian, Ubuntu, KDE neon | `sudo apt install python3-pyqt6` |

Get the code:

```sh
git clone https://github.com/Zana-C/ZTile.git
cd ZTile/linux
```

Then choose one of two ways to run it. Neither needs root, and neither touches
system folders.

**Option A: run from the folder (portable, like the Windows exe)**

```sh
./ztile.sh
```

The first run builds the program into `linux/target/release/`. Nothing is copied
anywhere else. If you move or delete the folder, ZTile is gone (turn off "Start on
login" first if you enabled it).

**Option B: install for your user**

```sh
./install.sh
```

This builds ZTile and copies it into your home folder, with an application menu entry:

| What | Where |
|---|---|
| Program | `~/.local/bin/ztile` |
| KWin script and settings window | `~/.local/share/ztile/` |
| Menu entry and icon | `~/.local/share/applications/io.github.ZanaC.ZTile.desktop`, `~/.local/share/icons/hicolor/scalable/apps/` |

Run `./install.sh` again to update, and `./install.sh --remove` to uninstall.
Uninstalling keeps your saved layouts in `~/.config/ztile`; delete that folder to
remove them too.

ZTile doesn't change your KWin settings. It loads its KWin script when it starts
and unloads it when it quits.

---

## Using ZTile

1. **Start ZTile.** The ZTile window opens and an icon appears in the system tray
   (on Windows it may be tucked under the `^` arrow).
2. **Click *New layout*.** Give it a name, pick a display and a preset.
3. **Choose the apps.** Click each cell to choose an app: one that's open right
   now, one specific window, or a program to start (on Windows an `.exe` or a
   Start menu shortcut, on Linux any installed app). The same menu splits or
   removes cells. Drag the lines between cells to resize them.
4. **Click *Save and open*.** The apps move into place, or start if they weren't
   running, and the group gets its own taskbar button.

From then on:

- **Open a layout** by clicking its card in the ZTile window, or from the tray
  icon's menu. If it's already open, it comes to the front.
- **Right-click a card** to edit, close or delete the layout.
- **Click the group's taskbar button** to bring the group up or minimize it.
  Minimizing any app in the group minimizes the whole group, because the apps no
  longer have taskbar buttons of their own.
- **Drag a window** onto another cell, out of the group, or into an empty cell.
- **Close the ZTile window** to keep ZTile running in the tray. Use **Quit** in
  the tray menu to exit; all groups are dissolved and every app gets its own
  taskbar button back.
- **Start automatically:** "Start with Windows" or "Start on login" in the tray menu.

---

## Command line

Everything is available from the window and the tray. For scripting:

| Command | What it does | Windows | Linux |
|---|---|---|---|
| `ztile` | Open the ZTile window (starts ZTile if needed) | ✓ | ✓ |
| `ztile --tray` | Start in the tray only | ✓ | ✓ |
| `ztile open <NAME>` | Open a saved layout (`--mon N` on Windows: on display N) | ✓ | ✓ |
| `ztile close <NAME>` | Dissolve an open group | – | ✓ |
| `ztile layouts` | List presets and saved layouts | ✓ | ✓ |
| `ztile list` | List displays and open windows (Windows) or open groups (Linux) | ✓ | ✓ |
| `ztile grid …` | Tile windows by title or exe name; see `ztile help` | ✓ | – |
| `ztile delete <NAME>` | Delete a saved layout | ✓ | – |
| `ztile quit` | Dissolve all groups and exit | ✓ | ✓ |

On Windows, ZTile is a windowed program, so the shell doesn't wait for it and
output can appear after the prompt; pipe it to see it in order, e.g.
`ztile list | more`. On Linux, use `./ztile.sh` instead of `ztile` if you didn't install it.

---

## Your data and privacy

ZTile has **no network code**, sends nothing anywhere and collects no telemetry.

**Saved layouts:**

| | Location | Per cell, ZTile keeps |
|---|---|---|
| Windows | `%APPDATA%\ZTile\layouts.json` | the app's executable path, its AppUserModelID, its window class and display name |
| Linux | `~/.config/ztile/layouts.json` (readable only by you) | the app's id (its `.desktop` name) and display name |

On both, a window title fragment is stored **only** if you pinned a cell to one
specific window. Window titles change all the time and often contain private
text, such as chat names, so they're not saved otherwise.

The file format is the same on both systems. App entries don't carry over between
Windows and Linux, because apps are identified differently.

**Other files and settings:**

- Windows: "Start with Windows" adds a value named `ZTile` under
  `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.
- Linux: "Start on login" adds `~/.config/autostart/io.github.ZanaC.ZTile.desktop`.
  While a group is open, a hidden `~/.local/share/applications/ztile-group-*.desktop`
  file and its icon in `~/.cache/ztile/icons/` give the group its own taskbar
  button. They're removed when the group closes, and any leftovers are cleaned up
  at the next start.

---

## Security

ZTile runs with your normal user rights, never asks for administrator/root access,
and never injects code into other programs.

**Starting programs.** A saved layout can't make ZTile run arbitrary commands:

- Windows: only an existing `.exe` on a local drive, or a valid AppUserModelID
  through the Start menu's app list. Scripts, documents, network paths (including
  network drives mapped to a letter) and NTFS alternate data streams are refused.
- Linux: only an app installed as a `.desktop` entry, found by its id (which may
  only contain letters, digits, `.`, `_` and `-`) and started with
  `kioclient exec`, exactly as the Plasma menu starts it: in its own systemd unit,
  so it isn't counted as part of ZTile and keeps running when ZTile quits. No shell
  is involved, and no command line from the layout file is ever executed.

**Untrusted input.** The layout file, command-line arguments and messages from
other processes are size- and depth-limited and type-checked before use. A
corrupt layout file is moved aside as `layouts.json.bad` instead of being
overwritten. Text from files or from other apps (layout names, window titles) is
always shown as plain text, never interpreted as HTML.

**Inter-process communication.**

- Windows: the running ZTile accepts commands (`open`, `show`, `quit`…) only
  through `WM_COPYDATA` in a fixed, size-checked format. Low-integrity processes
  are blocked by Windows (UIPI).
- Linux: the program and its KWin script talk over the session D-Bus
  (`io.github.ZanaC.ZTile`). The methods the KWin script uses (`event`, `poll`)
  accept calls **only from the KWin process**. Other programs (including the
  settings window) can use only the same harmless commands as the command line.
  The KWin script validates every command it receives.

**Files written on Linux** are created atomically (temporary file, then rename),
which also means a symlink planted at the target can't redirect the write. Values
written into `.desktop` files are escaped, so a layout name can't add keys such as
`Exec=`.

**Hardening of the Windows executable:** ASLR, DEP, Control Flow Guard and CET
shadow-stack compatibility. It loads its DLLs from System32 only, refuses DLLs
from remote or low-integrity locations, and turns off legacy DLL injection
points (AppInit DLLs and similar) at startup.

**Dependencies:**

- Windows: only Microsoft's official [`windows`](https://github.com/microsoft/windows-rs)
  crate (plus macro crates it uses at build time). Small parts such as JSON
  parsing are written in-house. Versions are pinned in `Cargo.lock`.
- Linux: the program uses a few widely used Rust crates (libdbus bindings,
  Wayland client, PNG and SVG decoding), pinned in `linux/Cargo.lock` and listed in
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). The settings window uses
  Python's standard library and PyQt6/Qt from your distribution.

**What ZTile does *not* protect against.** Programs running as *your* user
already have the same rights as ZTile:

- On Windows they can send window messages to other apps.
- On KDE Plasma they can load their own KWin scripts.

ZTile doesn't add a way around that, and it can't create a boundary that the
desktop itself doesn't have. On Windows, apps running as administrator can't be
moved by a non-elevated ZTile; that is a Windows security boundary.

Found a security problem? Please open a private
[security advisory](https://github.com/Zana-C/ZTile/security/advisories/new)
rather than a public issue.

---

## How it works

**Windows.** One process owns all groups. For each group:

- The member windows' taskbar buttons are hidden (`ITaskbarList::DeleteTab`).
- An always-minimized, invisible "representative" window with its own
  AppUserModelID owns the group's button and its composite icon. A click on that
  button arrives as a restore request, and ZTile turns it into a toggle.
- Windows are raised without being activated: briefly topmost, then not topmost.
- Window events come from out-of-process accessibility hooks (`SetWinEventHook`),
  and noisy events are watched only for the processes involved.

**KDE Plasma.** On Wayland only KWin may move other apps' windows, so the work is
split up:

- **A KWin script** (`linux/kwin/contents/code/main.js`) places windows, hides
  their taskbar entries (`skipTaskbar`), toggles and stacks groups, and handles
  drag and drop, display changes and newly started apps.
- **The `ztile` program** (Rust, no GUI toolkit) stays in the background. It loads
  the script and answers its long-poll D-Bus calls with commands, starts apps,
  keeps the saved layouts, serves the tray icon (StatusNotifierItem) directly over
  D-Bus, and owns a few tiny Wayland windows of its own:
  - one invisible 1×1 "proxy" window per group, which gets the group's taskbar
    button through a unique app id and a hidden `.desktop` file
  - the `+` placeholders in empty cells
  - the drop overlay shown while dragging

  Their content is a single-pixel buffer stretched by the compositor, so they
  cost almost no memory however large they are.
- **The settings window** (Python/PyQt6, `linux/ui/`) is started on demand, talks
  to `ztile` over D-Bus, and exits when you close it.
- While a group is up, the invisible proxy holds the keyboard focus, so no member
  app gets activated.
- **If `ztile` exits or crashes,** its proxy windows disappear. KWin notices and
  gives every window its own taskbar entry back.

---

## Known limitations

- **Windows:**
  - Windows of apps running as administrator can't be moved unless ZTile also runs
    as administrator.
  - If ZTile is force-killed (for example from Task Manager), the grouped apps'
    taskbar buttons stay hidden until Explorer restarts. Quitting normally
    restores them.
  - Open groups aren't moved when displays change (docking, resolution change).
- **Linux:** KDE Plasma 6 on Wayland only.
- **"Hide title bars"** only affects apps with a system-drawn title bar. Apps that
  draw their own keep theirs, because their buttons live there:
  - on Windows: most Chromium and Electron apps
  - on Linux: GTK apps and Chromium/Electron apps
- **Small cells:** some apps enforce a minimum size and may not fit.
- **Cell limit:** a layout can have up to 16 cells.

---

## Troubleshooting

| Problem | What to do |
|---|---|
| Windows: "Windows protected your PC" | *More info → Run anyway* (the exe isn't signed yet). |
| Windows: the tray icon isn't visible | Check under the `^` arrow on the taskbar, or pin it in the taskbar settings. |
| Linux: "KWin's scripting interface isn't available" | You're not running KDE Plasma 6, or KWin isn't the window manager. |
| Linux: "needs a KDE Plasma (Wayland) session" | Log in to the "Plasma (Wayland)" session; X11 isn't supported. |
| Linux: nothing happens when you click "Open ZTile" | The settings window needs PyQt6 (see Requirements); a notification says so. |
| Linux: `ztile` isn't found after installing | `~/.local/bin` isn't on your `PATH`. Start ZTile from the application menu, or run `~/.local/bin/ztile`. |
| Linux: an app isn't started when a layout opens | It needs a `.desktop` entry. Apps installed by your package manager or Flatpak have one; for others, pick a window of the running app instead. |
| An app ends up in the wrong cell or isn't recognized | Edit the layout and choose the app again from its open window. |
| A group's taskbar buttons are missing after a crash (Windows) | Restart Explorer, or start and quit ZTile once. |

---

## Building and testing

**Windows** (on Windows, in `windows/`):

```powershell
cargo test             # unit tests (layout model, JSON, app matching, store)
cargo build --release  # windows\target\release\ztile.exe
```

**Linux** (in `linux/`):

```sh
cargo test                                      # program: store, files, icons, ids
(cd ../core && cargo test)                      # shared layout model and JSON
(cd ui && python3 -m unittest discover -s tests) # settings window
./ztile.sh                                      # build and run from source
```

The KWin script has no build step. ZTile loads `kwin/contents/code/main.js` when it
starts. KWin's script messages appear in the journal: `journalctl --user -f | grep ztile`.

A debug build (`cargo build`) started with `ZTILE_DEV=1` uses its own bus name and
KWin script, so it can run next to an installed copy. Release builds ignore it.

---

## Repository layout

```text
ZTile/
├── README.md                  this file
├── LICENSE                    MIT
├── THIRD_PARTY_NOTICES.md
├── windows/                   Windows version (Rust)
│   ├── Cargo.toml, Cargo.lock, .cargo/config.toml (hardening flags)
│   ├── PLAN.md                design notes (Turkish)
│   └── src/
│       ├── main.rs            entry point, single instance, command line
│       ├── workspace.rs       live groups: tiling, taskbar button, drag & drop, launching
│       ├── ui.rs              main window and layout editor (GDI)
│       ├── tray.rs            tray icon, menu, start with Windows
│       ├── layout.rs          the shared layout model (core/) with Win32 rectangles
│       ├── apps.rs            identifying apps, matching windows, starting programs
│       ├── store.rs           saving and loading layouts
│       ├── win.rs             window helpers (enumeration, placement, DPI, displays)
│       ├── icon.rs            app icons and the group's composite icon
│       └── json.rs            the shared JSON reader/writer (core/)
├── core/                      shared by both versions (Rust, no dependencies)
│   └── src/
│       ├── layout.rs          layout model (split tree), presets, geometry
│       └── json.rs            minimal JSON reader/writer
└── linux/                     KDE Plasma version
    ├── ztile.sh               build and run from the folder
    ├── install.sh             install for your user / --remove
    ├── Cargo.toml, Cargo.lock
    ├── src/                   the `ztile` program (Rust)
    │   ├── main.rs            entry point, single instance, command line, event loop
    │   ├── daemon.rs          groups, the D-Bus link to KWin, starting apps, settings window
    │   ├── wl.rs              ZTile's own Wayland windows (group buttons, + cells, overlay)
    │   ├── tray.rs            tray icon and menu (StatusNotifierItem + dbusmenu)
    │   ├── bus.rs             D-Bus plumbing, KWin-only checks
    │   ├── apps.rs            .desktop lookup, names, icons, starting apps
    │   ├── icon.rs            ZTile's icon, group icons, the + glyph
    │   ├── store.rs           saving and loading layouts
    │   └── files.rs           atomic writes, .desktop escaping
    ├── kwin/                  the KWin script (window engine)
    ├── ui/                    the settings window (Python/PyQt6, runs only while open)
    │   ├── ztile_ui/          main window, layout editor, empty-cell picker
    │   └── tests/
    └── data/                  menu entry and icon
```

---

## License

ZTile is released under the [MIT License](LICENSE).

- The Windows executable includes code from Microsoft's Windows crates for Rust,
  also under the MIT License.
- The Linux program is built from MIT/Apache/BSD/Zlib-licensed Rust crates. Its
  settings window uses PyQt6 (GPL v3) and Qt (LGPL v3) from your distribution;
  ZTile doesn't bundle them.

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). If you redistribute
`ztile.exe`, include both files.

Windows is a trademark of the Microsoft group of companies, and KDE and Plasma are
trademarks of KDE e.V. Other app names mentioned here belong to their respective
owners. ZTile isn't affiliated with any of them.
