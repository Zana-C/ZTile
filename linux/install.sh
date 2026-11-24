#!/bin/sh
# Install ZTile for KDE Plasma into your home directory (no root needed).
#   ./install.sh            build (needs Rust) and install / update
#   ./install.sh --remove   uninstall (your saved layouts are kept)
set -eu

SRC=$(cd "$(dirname "$0")" && pwd)
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
DEST=$DATA/ztile
BIN=$HOME/.local/bin
APPS=$DATA/applications
ICONS=$DATA/icons/hicolor/scalable/apps
AUTOSTART=${XDG_CONFIG_HOME:-$HOME/.config}/autostart/io.github.ZanaC.ZTile.desktop

stop_running() {
    if [ -x "$BIN/ztile" ]; then "$BIN/ztile" quit >/dev/null 2>&1 && sleep 1 || true; fi
}

if [ "${1:-}" = "--remove" ]; then
    stop_running
    rm -rf "$DEST" "$BIN/ztile" "$APPS/io.github.ZanaC.ZTile.desktop" "$ICONS/io.github.ZanaC.ZTile.svg" "$AUTOSTART"
    command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true
    echo "ZTile removed. Saved layouts are still in ${XDG_CONFIG_HOME:-$HOME/.config}/ztile."
    exit 0
fi

command -v cargo >/dev/null 2>&1 || {
    echo "Building ZTile needs Rust (Arch/CachyOS: sudo pacman -S rustup && rustup default stable;" >&2
    echo "others: https://rustup.rs)." >&2
    exit 1
}
/usr/bin/python3 -c 'import PyQt6.QtDBus, PyQt6.QtWidgets' 2>/dev/null || {
    echo "Note: the settings window needs PyQt6 for the system Python (Arch/CachyOS: python-pyqt6," >&2
    echo "Fedora: python3-pyqt6, Debian/Ubuntu: python3-pyqt6). ZTile itself runs without it." >&2
}

(cd "$SRC" && cargo build --release --locked)

stop_running
rm -rf "$DEST"
mkdir -p "$DEST/kwin/contents/code" "$DEST/ui" "$BIN" "$APPS" "$ICONS"
install -m 755 "$SRC/target/release/ztile" "$BIN/ztile"
cp "$SRC/kwin/metadata.json" "$DEST/kwin/"
cp "$SRC/kwin/contents/code/main.js" "$DEST/kwin/contents/code/"
cp -r "$SRC/ui/ztile_ui" "$DEST/ui/"
rm -rf "$DEST/ui/ztile_ui/__pycache__"
cp "$SRC/data/io.github.ZanaC.ZTile.svg" "$ICONS/"
sed "s|^Exec=ztile|Exec=$BIN/ztile|" "$SRC/data/io.github.ZanaC.ZTile.desktop" > "$APPS/io.github.ZanaC.ZTile.desktop"
# an older autostart entry may point at a Python launcher: point it at the program
[ -f "$AUTOSTART" ] && sed -i "s|^Exec=.*|Exec=$BIN/ztile --tray|" "$AUTOSTART"
command -v kbuildsycoca6 >/dev/null 2>&1 && kbuildsycoca6 >/dev/null 2>&1 || true

echo "ZTile installed. Start it from the application menu or run: $BIN/ztile"
case ":$PATH:" in *":$BIN:"*) ;; *) echo "(Note: $BIN isn't on your PATH.)" ;; esac
