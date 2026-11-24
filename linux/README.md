# ZTile for KDE Plasma

The full documentation (features, installing, usage, data, security) is in the
[main README](../README.md). This file is a short reference for this folder.

## Run or install

```sh
./ztile.sh            # build (first time) and run from this folder; nothing is installed
./install.sh          # build and install for your user (~/.local), no root
./install.sh --remove # uninstall (saved layouts are kept)
```

Requires KDE Plasma 6 on Wayland and Rust to build. The settings window also needs
PyQt6 for the system Python (`python-pyqt6` / `python3-pyqt6`).

## What runs

| Part | Path | When |
|---|---|---|
| `ztile` (Rust) | `src/` | always, in the background: ~2 MB of memory, no CPU while idle |
| KWin script | `kwin/contents/code/main.js` | loaded into KWin while `ztile` runs |
| Settings window (Python/PyQt6) | `ui/ztile_ui/` | only while it's open |

## Protocol between `ztile` and the KWin script

Bus name `io.github.ZanaC.ZTile`, object `/ZTile`, interface `io.github.ZanaC.ZTile`:

- `poll() -> s`: the script's long poll; answered with a JSON array of commands
  (`config`, `open`, `close`, `closeAll`, `show`, `fill`, `launch`, `windows`),
  or `[]` every ~20 s. KWin only.
- `event(s)`: JSON events from the script (`hello`, `outputs`, `state`, `launch`,
  `overlay`, `windows`). KWin only.
- `command(s) -> s`: a JSON array of strings from the command line or the settings
  window (`show`, `open`, `close`, `quit`, `list`, `state`, `windows`, `cell`,
  `fill`, `launch`).
- Signals `Changed` (groups or displays changed) and `ShowWindow`.

## Tests

```sh
cargo test
(cd ../core && cargo test)
(cd ui && python3 -m unittest discover -s tests)
```
