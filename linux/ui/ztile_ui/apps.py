"""Installed apps (.desktop files) for the pickers: names and icons. Starting apps
is done by the ZTile service (its own .desktop lookup), not here.

A window's identity on Plasma is its app id (Wayland app_id, or the X11 class),
which normally is the name of its .desktop file: "org.kde.dolphin", "discord",
and browser web apps get their own ("brave-<id>-Default"). A cell stores that id.
"""

from __future__ import annotations

import locale
import os
from dataclasses import dataclass
from pathlib import Path

from PyQt6.QtGui import QIcon

MAX_ENTRY_BYTES = 256 * 1024


@dataclass
class DesktopApp:
    id: str  # desktop file id without ".desktop"
    path: str
    name: str
    icon: str
    wm_class: str
    hidden: bool  # NoDisplay: not shown in pickers, but still used for names/icons


def data_dirs() -> list[Path]:
    home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
    out = []
    for d in [home] + dirs.split(":"):
        p = Path(d) / "applications"
        if d and p.is_dir() and p not in out:
            out.append(p)
    return out


def _langs() -> list[str]:
    lang = os.environ.get("LC_MESSAGES") or os.environ.get("LANG") or locale.getlocale()[0] or ""
    lang = lang.split(".")[0].split("@")[0]
    return [x for x in (lang, lang.split("_")[0]) if x]


def _parse(path: Path) -> dict | None:
    try:
        if path.stat().st_size > MAX_ENTRY_BYTES:
            return None
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    out, section = {}, None
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("["):
            section = line
            continue
        if section == "[Desktop Entry]" and "=" in line:
            k, v = line.split("=", 1)
            out.setdefault(k.strip(), v.strip())
    return out


class AppIndex:
    """All .desktop applications, user entries first (they override system ones)."""

    def __init__(self):
        self.apps: dict[str, DesktopApp] = {}
        self._lower: dict[str, DesktopApp] = {}
        self._wm: dict[str, DesktopApp] = {}
        self._icons: dict[str, QIcon] = {}
        self.reload()

    def reload(self):
        self.apps.clear()
        self._icons.clear()
        langs = _langs()
        for base in data_dirs():
            for path in sorted(base.rglob("*.desktop")):
                did = str(path.relative_to(base))[: -len(".desktop")].replace("/", "-")
                if did in self.apps or did.startswith("ztile-group-"):
                    continue
                e = _parse(path)
                if not e or e.get("Type") != "Application" or e.get("Hidden") == "true":
                    continue
                name = next((e[f"Name[{l}]"] for l in langs if f"Name[{l}]" in e), e.get("Name", did))
                self.apps[did] = DesktopApp(
                    id=did,
                    path=str(path),
                    name=name,
                    icon=e.get("Icon", ""),
                    wm_class=e.get("StartupWMClass", ""),
                    hidden=e.get("NoDisplay") == "true",
                )
        self._lower = {k.lower(): v for k, v in self.apps.items()}
        self._wm = {v.wm_class.lower(): v for v in self.apps.values() if v.wm_class}

    def find(self, app_id: str) -> DesktopApp | None:
        """The .desktop entry for a window's app id."""
        if not app_id:
            return None
        k = app_id.lower()
        a = self._lower.get(k) or self._wm.get(k)
        if a:
            return a
        # reverse-DNS ids for apps that report a short class ("dolphin" -> org.kde.dolphin)
        for did, a in self._lower.items():
            if did.endswith("." + k):
                return a
        return None

    def name(self, app_id: str) -> str:
        a = self.find(app_id)
        return a.name if a else app_id

    def icon(self, app_id: str) -> QIcon:
        if app_id in self._icons:
            return self._icons[app_id]
        a = self.find(app_id)
        ic = QIcon()
        if a and a.icon:
            ic = QIcon(a.icon) if a.icon.startswith("/") else QIcon.fromTheme(a.icon)
        if ic.isNull():
            ic = QIcon.fromTheme(app_id.lower())
        if ic.isNull():
            ic = QIcon.fromTheme("application-x-executable")
        self._icons[app_id] = ic
        return ic

    def visible(self) -> list[DesktopApp]:
        return sorted((a for a in self.apps.values() if not a.hidden), key=lambda a: a.name.casefold())


_index: AppIndex | None = None


def index() -> AppIndex:
    global _index
    if _index is None:
        _index = AppIndex()
    return _index
