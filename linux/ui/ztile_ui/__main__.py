"""ZTile's settings window. Started by the ZTile service when you open it, and
gone (process and all) when you close it.

  python3 -m ztile_ui              the main window (layouts, editor)
  python3 -m ztile_ui --cell G C   fill empty cell C of open group G
"""

from __future__ import annotations

import sys
import traceback


def main():
    from PyQt6.QtWidgets import QApplication, QMessageBox

    from .client import APP_ID, DaemonClient

    # PyQt aborts on an exception in a slot; log it and carry on instead
    sys.excepthook = lambda t, v, tb: traceback.print_exception(t, v, tb)
    QApplication.setDesktopFileName(APP_ID)
    app = QApplication(sys.argv[:1])
    app.setApplicationName("ZTile")
    client = DaemonClient()
    if not client.available:
        QMessageBox.warning(None, "ZTile", "ZTile isn't running. Start it from the application menu.")
        sys.exit(1)
    client.gone.connect(app.quit)

    args = sys.argv[1:]
    if args[:1] == ["--cell"] and len(args) == 3 and all(a.isdigit() for a in args[1:]):
        from .ui import CellPicker

        CellPicker(client, int(args[1]), int(args[2])).exec()
        return

    from .ui import MainWindow

    w = MainWindow(client)

    def bring_up():
        w.show()
        w.raise_()
        w.activateWindow()

    client.show_requested.connect(bring_up)
    w.show()
    app.exec()


if __name__ == "__main__":
    main()
