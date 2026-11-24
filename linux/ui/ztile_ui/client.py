"""Talking to the ZTile service (the small resident `ztile` program) over D-Bus.

The settings window only runs while it's open; everything that lives on (groups,
the KWin script, the tray) belongs to the service.
"""

from __future__ import annotations

import json
import os

from PyQt6.QtCore import QObject, pyqtSignal, pyqtSlot
from PyQt6.QtDBus import QDBusConnection, QDBusInterface, QDBusPendingCallWatcher, QDBusReply, QDBusServiceWatcher

# ZTILE_DEV: the window was started by a development copy of the service
SERVICE = "io.github.ZanaC.ZTile.Dev" if os.environ.get("ZTILE_DEV") else "io.github.ZanaC.ZTile"
OBJECT = "/ZTile"
IFACE = "io.github.ZanaC.ZTile"
APP_ID = "io.github.ZanaC.ZTile"


class DaemonClient(QObject):
    changed = pyqtSignal()  # groups opened/closed, displays changed
    show_requested = pyqtSignal()  # "Open ZTile" while the window is already open
    gone = pyqtSignal()  # the service quit

    def __init__(self):
        super().__init__()
        self.bus = QDBusConnection.sessionBus()
        self.iface = QDBusInterface(SERVICE, OBJECT, IFACE, self.bus)
        self.outputs: list[dict] = []
        self._open: list[str] = []
        self.connected = False
        self.bus.connect(SERVICE, OBJECT, IFACE, "Changed", self._on_changed)
        self.bus.connect(SERVICE, OBJECT, IFACE, "ShowWindow", self._on_show)
        self.watcher = QDBusServiceWatcher(SERVICE, self.bus, QDBusServiceWatcher.WatchModeFlag.WatchForUnregistration, self)
        self.watcher.serviceUnregistered.connect(lambda *_: self.gone.emit())
        self.refresh()

    @property
    def available(self) -> bool:
        return self.bus.interface().isServiceRegistered(SERVICE).value()

    def call(self, *args: str) -> str:
        r = self.iface.call("command", json.dumps(list(args)))
        a = r.arguments()
        return a[0] if a and isinstance(a[0], str) else ""

    def refresh(self):
        try:
            st = json.loads(self.call("state") or "{}")
        except ValueError:
            st = {}
        self.outputs = [o for o in st.get("outputs", []) if isinstance(o, dict)]
        self._open = [n for n in st.get("open", []) if isinstance(n, str)]
        self.connected = st.get("connected") is True

    @pyqtSlot()
    def _on_changed(self):
        self.refresh()
        self.changed.emit()

    @pyqtSlot()
    def _on_show(self):
        self.show_requested.emit()

    # the same calls the editor made on the old in-process engine

    def open_names(self) -> list[str]:
        return list(self._open)

    def is_open(self, name: str) -> bool:
        return any(n.casefold() == name.casefold() for n in self._open)

    def open_saved(self, l):
        """Open (or bring up) a layout that is already saved to the layouts file."""
        self.call("open", l.name)

    def close_named(self, name: str):
        self.call("close", name)

    def request_windows(self, callback):
        """Open app windows (from KWin), without blocking the UI."""
        pending = self.iface.asyncCall("command", json.dumps(["windows"]))
        w = QDBusPendingCallWatcher(pending, self)

        def done(watcher):
            watcher.deleteLater()
            reply = QDBusReply(watcher)
            try:
                wins = json.loads(reply.value()) if reply.isValid() else []
            except (ValueError, TypeError):
                wins = []
            # never offer ZTile's own windows (this one)
            callback([x for x in wins if isinstance(x, dict) and x.get("app") != APP_ID])

        w.finished.connect(done)

    def cell(self, gid: int, cell: int) -> dict:
        try:
            return json.loads(self.call("cell", str(gid), str(cell)) or "{}")
        except ValueError:
            return {}

    def fill(self, gid: int, cell: int, win: str):
        self.call("fill", str(gid), str(cell), win)

    def launch(self, gid: int, cell: int, app: str):
        self.call("launch", str(gid), str(cell), app)
