"""Saved layouts: $XDG_CONFIG_HOME/ztile/layouts.json (usually ~/.config/ztile)."""

from __future__ import annotations

import copy
import json
import math
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

from . import layout
from .files import private_dir, write_atomic

VERSION = 1
MAX_FILE_BYTES = 1024 * 1024
MAX_LAYOUTS = 200


def config_dir() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return Path(base) / "ztile"


@dataclass
class AppSpec:
    """What a cell remembers about its app. No window titles by default: they change
    all the time and often contain private text (chat names...)."""

    app: str  # desktop file id / Wayland app id, e.g. "discord", "brave-<id>-Default"
    name: str = ""  # display name, e.g. "Discord"
    window: str = ""  # "" = any window of the app; else a title fragment the user picked

    def to_json(self):
        d = {"app": self.app, "name": self.name}
        if self.window:
            d["window"] = self.window
        return d

    @staticmethod
    def from_json(j):
        if not isinstance(j, dict) or not isinstance(j.get("app"), str) or not j["app"]:
            return None
        s = lambda k: j.get(k) if isinstance(j.get(k), str) else ""  # noqa: E731
        return AppSpec(j["app"][:256], s("name")[:256], s("window")[:256])

    def label(self) -> str:
        n = self.name or self.app
        return f"{n} – “{self.window[:30]}”" if self.window else n

    def for_engine(self):
        return {"app": self.app, "window": self.window}


@dataclass
class Monitor:
    """Enough to find the same display again: output name + physical size."""

    name: str = ""
    width: int = 0
    height: int = 0


@dataclass
class SavedLayout:
    name: str
    monitor: Monitor = field(default_factory=Monitor)
    tree: object = layout.CELL
    gap: int = 0
    borderless: bool = False
    # False (default): the layout only changes in the editor. True: changes made in
    # the open group (dragging, "+") are written back.
    keep_changes: bool = False
    apps: list = field(default_factory=list)  # one AppSpec | None per cell

    def clone(self) -> "SavedLayout":
        return copy.deepcopy(self)

    def to_json(self):
        return {
            "name": self.name,
            "monitor": {"name": self.monitor.name, "width": self.monitor.width, "height": self.monitor.height},
            "tree": layout.to_json(self.tree),
            "gap": self.gap,
            "borderless": self.borderless,
            "keep_changes": self.keep_changes,
            "apps": [a.to_json() if a else None for a in self.apps],
        }

    @staticmethod
    def from_json(j):
        if not isinstance(j, dict) or not isinstance(j.get("name"), str) or not j["name"].strip():
            return None
        tree = layout.from_json(j.get("tree"))
        if tree is None:
            return None
        m = j.get("monitor") if isinstance(j.get("monitor"), dict) else {}

        def num(v):
            return int(v) if isinstance(v, (int, float)) and math.isfinite(v) and 0 <= v < 100000 else 0

        gap = j.get("gap")
        gap = int(min(max(gap, 0), 64)) if isinstance(gap, (int, float)) and math.isfinite(gap) else 0
        apps = j.get("apps") if isinstance(j.get("apps"), list) else []
        n = layout.cell_count(tree)
        apps = [AppSpec.from_json(a) for a in apps[:n]] + [None] * max(0, n - len(apps))
        return SavedLayout(
            name=j["name"].strip()[:60],
            monitor=Monitor(str(m.get("name") or m.get("device") or "")[:64], num(m.get("width")), num(m.get("height"))),
            tree=tree,
            gap=gap,
            borderless=j.get("borderless") is True,
            keep_changes=j.get("keep_changes") is True,
            apps=apps,
        )


class Store:
    def __init__(self, layouts=None):
        self.layouts: list[SavedLayout] = layouts or []

    @staticmethod
    def path() -> Path:
        return config_dir() / "layouts.json"

    @classmethod
    def load(cls) -> "Store":
        """Missing file = empty store. A file we can read but can't parse is kept aside
        as .bad; a file we merely can't read right now is left alone."""
        p = cls.path()
        try:
            if p.stat().st_size > MAX_FILE_BYTES:
                raise ValueError(f"file larger than {MAX_FILE_BYTES // 1024} KB")
            text = p.read_text(encoding="utf-8-sig")
        except FileNotFoundError:
            return cls()
        except OSError as e:
            print(f"ztile: couldn't read {p}: {e}", file=sys.stderr)
            return cls()
        except ValueError as e:
            return cls._bad(p, e)
        try:
            j = json.loads(text)
            items = j["layouts"]
            if not isinstance(items, list):
                raise ValueError('"layouts" is not a list')
        except (ValueError, KeyError, TypeError, RecursionError) as e:
            return cls._bad(p, e)
        out = []
        for item in items[:MAX_LAYOUTS]:
            l = SavedLayout.from_json(item)
            if l and not any(x.name.casefold() == l.name.casefold() for x in out):
                out.append(l)
        return cls(out)

    @staticmethod
    def _bad(p: Path, e):
        bad = p.with_suffix(".json.bad")
        try:
            p.replace(bad)
            print(f"ztile: couldn't read {p} ({e}); moved it aside as {bad}.", file=sys.stderr)
        except OSError:
            pass
        return Store()

    def save(self):
        """Write a temp file, then rename over the old one: never half-written."""
        p = self.path()
        private_dir(p.parent)
        data = {"version": VERSION, "layouts": [l.to_json() for l in self.layouts]}
        # readable by the user only: a pinned cell keeps a window title fragment
        write_atomic(p, json.dumps(data, indent=2, ensure_ascii=False) + "\n", 0o600)

    def get(self, name: str):
        return next((l for l in self.layouts if l.name.casefold() == name.casefold()), None)

    def put(self, l: SavedLayout):
        for i, x in enumerate(self.layouts):
            if x.name.casefold() == l.name.casefold():
                self.layouts[i] = l
                return
        self.layouts.append(l)

    def remove(self, name: str) -> bool:
        n = len(self.layouts)
        self.layouts = [l for l in self.layouts if l.name.casefold() != name.casefold()]
        return len(self.layouts) != n
