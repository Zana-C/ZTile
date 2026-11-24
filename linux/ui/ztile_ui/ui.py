"""Main window: saved layouts as cards, and the layout editor. Plain Qt widgets, so
it follows the Plasma theme (light/dark, accent colour, fonts)."""

from __future__ import annotations

import copy
from html import escape

from PyQt6.QtCore import QPoint, QPointF, QRectF, QSize, Qt, pyqtSignal
from PyQt6.QtGui import QAction, QColor, QFont, QIcon, QPainter, QPalette, QPen, QPixmap
from PyQt6.QtWidgets import (
    QButtonGroup,
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMenu,
    QMessageBox,
    QPushButton,
    QScrollArea,
    QSizePolicy,
    QSpinBox,
    QStackedWidget,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from . import apps, icons, layout
from .store import AppSpec, Monitor, SavedLayout, Store


def pal(role: QPalette.ColorRole) -> QColor:
    from PyQt6.QtWidgets import QApplication

    return QApplication.palette().color(role)


def blend(a: QColor, b: QColor, t: float) -> QColor:
    return QColor(
        round(a.red() + (b.red() - a.red()) * t),
        round(a.green() + (b.green() - a.green()) * t),
        round(a.blue() + (b.blue() - a.blue()) * t),
    )


def fit(r: QRectF, aspect: float) -> QRectF:
    """Largest rect of the given aspect ratio centred in r."""
    w, h = r.width(), r.height()
    if h <= 0 or w <= 0:
        return r
    fw, fh = (h * aspect, h) if w / h > aspect else (w, w / aspect)
    return QRectF(r.x() + (w - fw) / 2, r.y() + (h - fh) / 2, fw, fh)


def aspect_of(m: Monitor | dict | None) -> float:
    if isinstance(m, dict):
        w, h = m.get("width", 0), m.get("height", 0)
    elif m:
        w, h = m.width, m.height
    else:
        w = h = 0
    return w / h if w > 0 and h > 0 else 16 / 10


def find_output(outputs: list[dict], m: Monitor) -> int:
    """Index of the output a saved monitor maps to (same rules as the KWin script)."""
    same = lambda o: o.get("width") == m.width and o.get("height") == m.height  # noqa: E731
    for test in (lambda o: o.get("name") == m.name and same(o), same, lambda o: o.get("name") == m.name,
                 lambda o: o.get("active")):
        for i, o in enumerate(outputs):
            if test(o):
                return i
    return 0


def output_label(i: int, o: dict) -> str:
    return f"{i + 1} · {o.get('width')}×{o.get('height')}"


def draw_layout(p: QPainter, area: QRectF, tree, specs, gap: float, big: bool, hover: int = -1):
    """Draw a layout's cells into area. Returns the cell rects."""
    idx = apps.index()
    base = pal(QPalette.ColorRole.Base)
    text = pal(QPalette.ColorRole.Text)
    cell_col = blend(base, text, 0.16)
    empty_col = blend(base, text, 0.05)
    hover_col = blend(base, pal(QPalette.ColorRole.Highlight), 0.35)
    rects = []
    a = (area.x(), area.y(), area.width(), area.height())
    cs = layout.cells(tree)
    for i, (x0, y0, x1, y1) in enumerate(cs):
        e = lambda at: gap if at else gap / 2  # noqa: E731
        r = QRectF(
            a[0] + x0 * a[2] + e(x0 <= 0),
            a[1] + y0 * a[3] + e(y0 <= 0),
            0,
            0,
        )
        r.setRight(a[0] + x1 * a[2] - e(x1 >= 1))
        r.setBottom(a[1] + y1 * a[3] - e(y1 >= 1))
        rects.append(r)
        spec = specs[i] if specs is not None and i < len(specs) else None
        col = hover_col if big and i == hover else (cell_col if spec or specs is None else empty_col)
        p.setPen(QPen(blend(base, text, 0.25), 1) if big else Qt.PenStyle.NoPen)
        p.setBrush(col)
        rad = 8 if big else 3
        p.drawRoundedRect(r, rad, rad)
        if spec:
            size = int(min(r.width(), r.height()) * (0.28 if big else 0.5))
            size = max(10, min(48, size))
            label_h = 22 if big else 0
            x = r.center().x() - size / 2
            y = r.y() + (r.height() - size - label_h) / 2
            idx.icon(spec.app).paint(p, QRectF(x, y, size, size).toRect())
            if big:
                p.setPen(text)
                f = QFont(p.font())
                f.setPointSizeF(f.pointSizeF() * 0.9)
                p.setFont(f)
                lr = QRectF(r.x() + 8, y + size + 4, r.width() - 16, label_h)
                p.drawText(lr, Qt.AlignmentFlag.AlignCenter,
                           p.fontMetrics().elidedText(spec.label(), Qt.TextElideMode.ElideRight, int(lr.width())))
        elif big:
            sub = blend(base, text, 0.55)
            p.setPen(sub)
            f = QFont(p.font())
            f.setPointSizeF(f.pointSizeF() * 2.4)
            f.setWeight(QFont.Weight.Light)
            p.save()
            p.setFont(f)
            mid = r.center().y()
            p.drawText(QRectF(r.x(), mid - 40, r.width(), 44), Qt.AlignmentFlag.AlignCenter, "+")
            p.restore()
            p.drawText(QRectF(r.x(), mid + 6, r.width(), 22), Qt.AlignmentFlag.AlignCenter, "Choose an app")
    return rects


def preset_icon(name: str, size: QSize) -> QIcon:
    pm = QPixmap(size)
    pm.fill(Qt.GlobalColor.transparent)
    p = QPainter(pm)
    p.setRenderHint(QPainter.RenderHint.Antialiasing)
    draw_layout(p, QRectF(1, 1, size.width() - 2, size.height() - 2), layout.preset(name), None, 2, False)
    p.end()
    return QIcon(pm)


# ---------------------------------------------------------------- app picker


class AppPicker(QDialog):
    """Search installed apps (.desktop entries)."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Choose an app")
        self.resize(420, 520)
        lay = QVBoxLayout(self)
        self.search = QLineEdit(placeholderText="Search apps…")
        self.search.setClearButtonEnabled(True)
        self.list = QListWidget()
        self.list.setIconSize(QSize(24, 24))
        lay.addWidget(self.search)
        lay.addWidget(self.list)
        bb = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        bb.accepted.connect(self.accept)
        bb.rejected.connect(self.reject)
        lay.addWidget(bb)
        idx = apps.index()
        for a in idx.visible():
            it = QListWidgetItem(idx.icon(a.id), a.name)
            it.setData(Qt.ItemDataRole.UserRole, a.id)
            it.setToolTip(escape(a.id))
            self.list.addItem(it)
        self.search.textChanged.connect(self._filter)
        self.list.itemDoubleClicked.connect(lambda *_: self.accept())
        self.search.returnPressed.connect(self.accept)
        if self.list.count():
            self.list.setCurrentRow(0)

    def _filter(self, t):
        t = t.casefold()
        first = None
        for i in range(self.list.count()):
            it = self.list.item(i)
            show = t in it.text().casefold() or t in str(it.data(Qt.ItemDataRole.UserRole)).casefold()
            it.setHidden(not show)
            if show and first is None:
                first = it
        if first:
            self.list.setCurrentItem(first)

    @staticmethod
    def pick(parent):
        d = AppPicker(parent)
        if d.exec() != QDialog.DialogCode.Accepted or not d.list.currentItem() or d.list.currentItem().isHidden():
            return None
        return apps.index().apps.get(d.list.currentItem().data(Qt.ItemDataRole.UserRole))


# ---------------------------------------------------------------- home


class LayoutCard(QWidget):
    W, H = 250, 216
    activated = pyqtSignal(object)
    menu_requested = pyqtSignal(object, QPoint)

    def __init__(self, l: SavedLayout, is_open: bool, display: str):
        super().__init__()
        self.l, self.is_open, self.display = l, is_open, display
        self.setFixedSize(self.W, self.H)
        self.setAttribute(Qt.WidgetAttribute.WA_Hover)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setToolTip(escape(f"Open “{l.name}”"))  # tooltips render HTML

    def paintEvent(self, e):
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        base = pal(QPalette.ColorRole.Base)
        text = pal(QPalette.ColorRole.Text)
        hot = self.underMouse()
        p.setPen(QPen(blend(base, text, 0.18), 1))
        p.setBrush(blend(base, pal(QPalette.ColorRole.Highlight), 0.12) if hot else base)
        p.drawRoundedRect(QRectF(self.rect()).adjusted(0.5, 0.5, -0.5, -0.5), 10, 10)
        pbox = QRectF(14, 14, self.W - 28, 128)
        screen = fit(pbox, aspect_of(self.l.monitor))
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(pal(QPalette.ColorRole.Window))
        p.drawRoundedRect(screen, 6, 6)
        draw_layout(p, screen.adjusted(4, 4, -4, -4), self.l.tree, self.l.apps, 3, False)
        p.setPen(text)
        f = QFont(self.font())
        f.setBold(True)
        f.setPointSizeF(f.pointSizeF() * 1.1)
        p.setFont(f)
        nr = QRectF(16, pbox.bottom() + 10, self.W - 32, 24)
        p.drawText(nr, Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter,
                   p.fontMetrics().elidedText(self.l.name, Qt.TextElideMode.ElideRight, int(nr.width())))
        p.setFont(self.font())
        n = sum(1 for a in self.l.apps if a)
        info = f"{n} app{'s' if n != 1 else ''} · {self.display}"
        p.setPen(blend(base, text, 0.6))
        ir = QRectF(16, nr.bottom(), self.W - 36, 22)
        p.drawText(ir, Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter, info)
        if self.is_open:
            acc = pal(QPalette.ColorRole.Highlight)
            p.setPen(acc)
            p.drawText(ir, Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter, "open")
            w = p.fontMetrics().horizontalAdvance("open")
            p.setBrush(acc)
            p.setPen(Qt.PenStyle.NoPen)
            p.drawEllipse(QPointF(ir.right() - w - 8, ir.center().y()), 3, 3)

    def mouseReleaseEvent(self, e):
        if e.button() == Qt.MouseButton.LeftButton and self.rect().contains(e.position().toPoint()):
            self.activated.emit(self.l)

    def contextMenuEvent(self, e):
        self.menu_requested.emit(self.l, e.globalPos())


class CardGrid(QWidget):
    """Wrapping grid of cards (scrolls inside a QScrollArea)."""

    GAP = 16

    def __init__(self):
        super().__init__()
        self.cards: list[QWidget] = []

    def set_cards(self, cards):
        for c in self.cards:
            c.deleteLater()
        self.cards = cards
        for c in cards:
            c.setParent(self)
            c.show()
        self._relayout()

    def resizeEvent(self, e):
        self._relayout()

    def _relayout(self):
        cw, ch = LayoutCard.W, LayoutCard.H
        cols = max(1, (self.width() + self.GAP) // (cw + self.GAP))
        for i, c in enumerate(self.cards):
            c.move((i % cols) * (cw + self.GAP), (i // cols) * (ch + self.GAP))
        rows = (len(self.cards) + cols - 1) // cols
        self.setMinimumHeight(max(0, rows * (ch + self.GAP) - self.GAP))


class Home(QWidget):
    new_layout = pyqtSignal()

    def __init__(self, main: "MainWindow"):
        super().__init__()
        self.main = main
        lay = QVBoxLayout(self)
        lay.setContentsMargins(28, 24, 28, 12)
        head = QHBoxLayout()
        titles = QVBoxLayout()
        t = QLabel("ZTile")
        f = t.font()
        f.setPointSizeF(f.pointSizeF() * 1.9)
        f.setBold(True)
        t.setFont(f)
        sub = QLabel("Saved layouts")
        sub.setEnabled(False)
        titles.addWidget(t)
        titles.addWidget(sub)
        head.addLayout(titles)
        head.addStretch()
        nb = QPushButton(QIcon.fromTheme("list-add"), " New layout")
        nb.setMinimumHeight(36)
        nb.clicked.connect(self.new_layout)
        head.addWidget(nb, alignment=Qt.AlignmentFlag.AlignTop)
        lay.addLayout(head)
        lay.addSpacing(12)
        self.grid = CardGrid()
        self.scroll = QScrollArea()
        self.scroll.setWidgetResizable(True)
        self.scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        holder = QWidget()
        hl = QVBoxLayout(holder)
        hl.setContentsMargins(0, 0, 0, 0)
        self.empty = QLabel(
            "<h3>No saved layouts yet</h3>Create your first one with “New layout” in the top right."
        )
        self.empty.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.empty.setEnabled(False)
        hl.addWidget(self.empty)
        hl.addWidget(self.grid)
        hl.addStretch()
        self.scroll.setWidget(holder)
        lay.addWidget(self.scroll, 1)
        foot = QLabel("Click a card to open it  ·  Right-click to edit, close or delete")
        foot.setEnabled(False)
        lay.addWidget(foot)

    def refresh(self, store: Store, open_names, outputs):
        cards = []
        for l in store.layouts:
            is_open = any(n.casefold() == l.name.casefold() for n in open_names)
            disp = f"Display {find_output(outputs, l.monitor) + 1}" if outputs else (l.monitor.name or "Display")
            c = LayoutCard(l, is_open, disp)
            c.activated.connect(self.main.open_layout)
            c.menu_requested.connect(self.main.card_menu)
            cards.append(c)
        self.grid.set_cards(cards)
        self.empty.setVisible(not cards)


# ---------------------------------------------------------------- editor


class Canvas(QWidget):
    """Big preview of the layout being edited: click a cell, drag a divider."""

    cell_clicked = pyqtSignal(int, QPoint)
    changed = pyqtSignal()

    def __init__(self, editor: "Editor"):
        super().__init__()
        self.ed = editor
        self.setMouseTracking(True)
        self.setMinimumHeight(200)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Expanding)
        self.hover_cell = -1
        self.hover_div = -1
        self.dragging = None
        self.cells: list[QRectF] = []
        self.inner = QRectF()

    def _geometry(self):
        screen = fit(QRectF(self.rect()).adjusted(1, 1, -1, -1), self.ed.aspect())
        return screen, screen.adjusted(10, 10, -10, -10)

    def _divider_rects(self):
        _, c = self._geometry()
        out = []
        for d in layout.dividers(self.ed.tree):
            if d.dir == "cols":
                x = c.x() + d.pos * c.width()
                out.append(QRectF(x - 6, c.y() + d.parent[1] * c.height(), 12, (d.parent[3] - d.parent[1]) * c.height()))
            else:
                y = c.y() + d.pos * c.height()
                out.append(QRectF(c.x() + d.parent[0] * c.width(), y - 6, (d.parent[2] - d.parent[0]) * c.width(), 12))
        return out

    def paintEvent(self, e):
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)
        screen, inner = self._geometry()
        self.inner = inner
        base = pal(QPalette.ColorRole.Base)
        p.setPen(QPen(blend(base, pal(QPalette.ColorRole.Text), 0.18), 1))
        p.setBrush(base)
        p.drawRoundedRect(screen, 10, 10)
        self.cells = draw_layout(p, inner, self.ed.tree, self.ed.apps, 6, True, self.hover_cell)
        acc = pal(QPalette.ColorRole.Highlight)
        divs = layout.dividers(self.ed.tree)
        for k, r in enumerate(self._divider_rects()):
            if k == self.hover_div or (self.dragging and self.dragging.path == divs[k].path and self.dragging.k == divs[k].k):
                p.setPen(Qt.PenStyle.NoPen)
                p.setBrush(acc)
                line = QRectF(r.center().x() - 2, r.y(), 4, r.height()) if divs[k].dir == "cols" else \
                    QRectF(r.x(), r.center().y() - 2, r.width(), 4)
                p.drawRoundedRect(line, 2, 2)

    def mouseMoveEvent(self, e):
        pos = e.position()
        if self.dragging:
            c = self.inner
            v = (pos.x() - c.x()) / max(1, c.width()) if self.dragging.dir == "cols" else \
                (pos.y() - c.y()) / max(1, c.height())
            layout.move_divider(self.ed.tree, self.dragging, min(max(v, 0.0), 1.0))
            d = self.dragging
            self.dragging = next((x for x in layout.dividers(self.ed.tree) if x.path == d.path and x.k == d.k), None)
            self.update()
            return
        hd = next((k for k, r in enumerate(self._divider_rects()) if r.contains(pos)), -1)
        hc = -1 if hd >= 0 else next((i for i, r in enumerate(self.cells) if r.contains(pos)), -1)
        if (hd, hc) != (self.hover_div, self.hover_cell):
            self.hover_div, self.hover_cell = hd, hc
            if hd >= 0:
                d = layout.dividers(self.ed.tree)[hd]
                self.setCursor(Qt.CursorShape.SizeHorCursor if d.dir == "cols" else Qt.CursorShape.SizeVerCursor)
            elif hc >= 0:
                self.setCursor(Qt.CursorShape.PointingHandCursor)
            else:
                self.unsetCursor()
            self.update()

    def leaveEvent(self, e):
        if not self.dragging:
            self.hover_div = self.hover_cell = -1
            self.update()

    def mousePressEvent(self, e):
        if e.button() == Qt.MouseButton.LeftButton and self.hover_div >= 0:
            self.dragging = layout.dividers(self.ed.tree)[self.hover_div]

    def mouseReleaseEvent(self, e):
        if self.dragging:
            self.dragging = None
            self.update()
            return
        if e.button() in (Qt.MouseButton.LeftButton, Qt.MouseButton.RightButton):
            i = next((i for i, r in enumerate(self.cells) if r.contains(e.position())), -1)
            if i >= 0:
                self.cell_clicked.emit(i, e.position().toPoint())


class Editor(QWidget):
    done = pyqtSignal()

    def __init__(self, main: "MainWindow", l: SavedLayout | None):
        super().__init__()
        self.main = main
        self.original = l.name if l else None
        self.outputs = list(main.engine.outputs)
        if l:
            self.tree = copy.deepcopy(l.tree)
            self.apps = list(l.apps)
            self.monitor = find_output(self.outputs, l.monitor)
        else:
            self.tree = layout.preset("2x2")
            self.apps = [None] * 4
            self.monitor = next((i for i, o in enumerate(self.outputs) if o.get("active")), 0)

        lay = QVBoxLayout(self)
        lay.setContentsMargins(28, 20, 28, 20)
        top = QHBoxLayout()
        back = QToolButton()
        back.setIcon(QIcon.fromTheme("go-previous"))
        back.setText("Back")
        back.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextBesideIcon)
        back.setAutoRaise(True)
        back.clicked.connect(self.done)
        top.addWidget(back)
        title = QLabel("Edit layout" if l else "New layout")
        f = title.font()
        f.setBold(True)
        f.setPointSizeF(f.pointSizeF() * 1.25)
        title.setFont(f)
        top.addWidget(title)
        top.addStretch()
        lay.addLayout(top)

        row = QHBoxLayout()
        row.addWidget(QLabel("Name"))
        self.name = QLineEdit(l.name if l else "")
        self.name.setMaxLength(60)
        self.name.setPlaceholderText("e.g. Chat, Work")
        self.name.setMinimumWidth(240)
        row.addWidget(self.name)
        row.addSpacing(24)
        row.addWidget(QLabel("Display"))
        self.mon_group = QButtonGroup(self)
        for i, o in enumerate(self.outputs):
            b = QPushButton(output_label(i, o))
            b.setToolTip(escape(f"{o.get('name')} · {o.get('label') or ''}"))
            b.setCheckable(True)
            b.setChecked(i == self.monitor)
            self.mon_group.addButton(b, i)
            row.addWidget(b)
        if not self.outputs:
            na = QLabel("(waiting for KWin…)")
            na.setEnabled(False)
            row.addWidget(na)
        self.mon_group.idClicked.connect(self._set_monitor)
        row.addStretch()
        lay.addLayout(row)

        pr = QHBoxLayout()
        pr.addWidget(QLabel("Presets"))
        for name in layout.PRESETS:
            b = QToolButton()
            b.setIcon(preset_icon(name, QSize(48, 32)))
            b.setIconSize(QSize(48, 32))
            b.setToolTip(name)
            b.clicked.connect(lambda _=False, n=name: self._preset(n))
            pr.addWidget(b)
        pr.addStretch()
        lay.addLayout(pr)

        self.canvas = Canvas(self)
        self.canvas.cell_clicked.connect(self._cell_menu)
        lay.addWidget(self.canvas, 1)

        opts = QHBoxLayout()
        col = QVBoxLayout()
        self.borderless = QCheckBox("Hide title bars (apps with a KDE-drawn title bar only)")
        self.borderless.setChecked(bool(l and l.borderless))
        self.keep = QCheckBox("Save changes made in the open group (dragging, +) to this layout")
        self.keep.setChecked(bool(l and l.keep_changes))
        col.addWidget(self.borderless)
        col.addWidget(self.keep)
        opts.addLayout(col)
        opts.addStretch()
        opts.addWidget(QLabel("Gap"))
        self.gap = QSpinBox()
        self.gap.setRange(0, 40)
        self.gap.setSingleStep(2)
        self.gap.setSuffix(" px")
        self.gap.setValue(l.gap if l else 0)
        opts.addWidget(self.gap)
        lay.addLayout(opts)

        bot = QHBoxLayout()
        self.status = QLabel("Click a cell to choose an app, split or remove it  ·  Drag a divider to resize")
        self.status.setEnabled(False)
        self.status.setWordWrap(True)
        self.status.setTextFormat(Qt.TextFormat.PlainText)  # messages can contain a layout name
        bot.addWidget(self.status, 1)
        cancel = QPushButton("Cancel")
        cancel.clicked.connect(self.done)
        save = QPushButton("Save")
        save.clicked.connect(lambda: self._save(False))
        so = QPushButton(QIcon.fromTheme("window-new"), "Save and open")
        so.setDefault(True)
        so.clicked.connect(lambda: self._save(True))
        for b in (cancel, save, so):
            bot.addWidget(b)
        lay.addLayout(bot)
        self.name.setFocus()

    def aspect(self):
        return aspect_of(self.outputs[self.monitor] if self.monitor < len(self.outputs) else None)

    def _set_monitor(self, i):
        self.monitor = i
        self.canvas.update()

    def _preset(self, name):
        self.tree = layout.preset(name)
        self.apps = (self.apps + [None] * 16)[: layout.cell_count(self.tree)]
        self.canvas.update()

    def error(self, msg):
        self.status.setText(msg)
        self.status.setEnabled(True)
        self.status.setStyleSheet("color: #e5534b;")

    def _cell_menu(self, i, pos):
        gpos = self.canvas.mapToGlobal(pos)
        self.main.engine.request_windows(lambda wins: self._show_cell_menu(i, gpos, wins))

    def _show_cell_menu(self, i, gpos, wins):
        if i >= len(self.apps):
            return
        idx = apps.index()
        cur = self.apps[i]
        m = QMenu(self)
        if cur:
            m.addAction(idx.icon(cur.app), f"Now: {cur.label()}".replace("&", "&&")).setEnabled(False)
            if cur.window:
                a = m.addAction(f"Match any {cur.name or cur.app} window")
                a.triggered.connect(lambda: self._set(i, AppSpec(cur.app, cur.name, "")))
            m.addSeparator()
        m.addAction("Open apps:").setEnabled(False)
        seen = []
        for w in wins:
            app = w.get("app") or ""
            if app and app.lower() not in seen:
                seen.append(app.lower())
                a = m.addAction(idx.icon(app), idx.name(app).replace("&", "&&"))
                a.triggered.connect(lambda _=False, app=app: self._set(i, AppSpec(app, idx.name(app))))
        if not seen:
            m.addAction("(none)").setEnabled(False)
        sub = m.addMenu("A specific window")
        for w in wins:
            app = w.get("app") or ""
            cap = w.get("caption") or ""
            if not app:
                continue
            a = sub.addAction(idx.icon(app), f"{short(cap, 55)}\t{idx.name(app)}".replace("&", "&&"))
            a.triggered.connect(lambda _=False, app=app, cap=cap: self._set(i, AppSpec(app, idx.name(app), title_filter(cap))))
        sub.setEnabled(bool(wins))
        pick = m.addAction(QIcon.fromTheme("search"), "Choose an app…")
        pick.triggered.connect(lambda: self._pick(i))
        m.addSeparator()
        if cur:
            m.addAction("Clear cell").triggered.connect(lambda: self._set(i, None))
        m.addAction(QIcon.fromTheme("view-split-left-right"), "Split side by side").triggered.connect(lambda: self._split(i, "cols"))
        m.addAction(QIcon.fromTheme("view-split-top-bottom"), "Split top and bottom").triggered.connect(lambda: self._split(i, "rows"))
        if layout.cell_count(self.tree) > 1:
            m.addAction(QIcon.fromTheme("edit-delete"), "Remove cell").triggered.connect(lambda: self._remove(i))
        m.popup(gpos)

    def _set(self, i, spec):
        if i < len(self.apps):
            self.apps[i] = spec
            self.canvas.update()

    def _pick(self, i):
        a = AppPicker.pick(self)
        if a:
            self._set(i, AppSpec(a.id, a.name))

    def _split(self, i, d):
        t = layout.split_cell(copy.deepcopy(self.tree), i, d)
        if t is None:
            self.error(f"A layout can have at most {layout.MAX_CELLS} cells.")
            return
        self.tree = t
        self.apps.insert(i + 1, None)
        self.apps = self.apps[: layout.cell_count(t)]
        self.canvas.update()

    def _remove(self, i):
        t = layout.remove_cell(copy.deepcopy(self.tree), i)
        if t is not None:
            self.tree = t
            del self.apps[i]
            self.canvas.update()

    def _save(self, open_after):
        name = self.name.text().strip()
        if not name:
            self.error("Give the layout a name.")
            return
        st = Store.load()
        renamed = self.original is None or self.original.casefold() != name.casefold()
        if renamed and st.get(name):
            self.error(f"A layout named “{name}” already exists.")
            return
        if self.monitor >= len(self.outputs):
            mon = Monitor()
        else:
            o = self.outputs[self.monitor]
            mon = Monitor(o.get("name", ""), o.get("width", 0), o.get("height", 0))
        l = SavedLayout(name, mon, copy.deepcopy(self.tree), self.gap.value(), self.borderless.isChecked(),
                        self.keep.isChecked(), list(self.apps))
        if self.original and renamed:
            st.remove(self.original)
        st.put(l)
        try:
            st.save()
        except OSError as e:
            self.error(f"Couldn't save: {e}")
            return
        eng = self.main.engine
        was_open = eng.is_open(name) or (self.original is not None and eng.is_open(self.original))
        if self.original:
            eng.close_named(self.original)
        eng.close_named(name)
        if open_after or was_open:
            eng.open_saved(l)
        if open_after:
            self.main.close()
        else:
            self.done.emit()


def ask(parent, text: str, question: bool = True) -> bool:
    """Message box that shows text as plain text (a layout name is not HTML)."""
    box = QMessageBox(QMessageBox.Icon.Question if question else QMessageBox.Icon.Warning, "ZTile", "", parent=parent)
    box.setTextFormat(Qt.TextFormat.PlainText)
    box.setText(text)
    if question:
        box.setStandardButtons(QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No)
        box.setDefaultButton(QMessageBox.StandardButton.No)
    return box.exec() == QMessageBox.StandardButton.Yes


def short(s: str, n: int) -> str:
    return s if len(s) <= n else s[: n - 3] + "..."


def title_filter(title: str) -> str:
    """A window title made into a stable filter: drop unread counters like "(3) "."""
    t = title.strip()
    if t.startswith("("):
        num, sep, after = t[1:].partition(")")
        if sep and num and all(c.isdigit() or c == "+" for c in num):
            return after.strip()
    return t


# ---------------------------------------------------------------- window


class MainWindow(QWidget):
    def __init__(self, engine):
        super().__init__()
        self.engine = engine
        self.setWindowTitle("ZTile")
        self.setWindowIcon(icons.app_icon())
        self.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
        self.resize(1000, 700)
        self.setMinimumSize(820, 560)
        self.stack = QStackedWidget()
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.addWidget(self.stack)
        self.home = Home(self)
        self.home.new_layout.connect(lambda: self.edit(None))
        self.stack.addWidget(self.home)
        self.editor: Editor | None = None
        esc = QAction(self)
        esc.setShortcut(Qt.Key.Key_Escape)
        esc.triggered.connect(self._escape)
        self.addAction(esc)
        engine.changed.connect(self.refresh)
        self.refresh()

    def refresh(self):
        self.home.refresh(Store.load(), self.engine.open_names(), self.engine.outputs)

    def _escape(self):
        if self.editor:
            self.leave_editor()
        else:
            self.close()

    def edit(self, l):
        self.leave_editor()
        self.editor = Editor(self, l)
        self.editor.done.connect(self.leave_editor)
        self.stack.addWidget(self.editor)
        self.stack.setCurrentWidget(self.editor)

    def leave_editor(self):
        if self.editor:
            self.stack.removeWidget(self.editor)
            self.editor.deleteLater()
            self.editor = None
        self.stack.setCurrentWidget(self.home)
        self.refresh()

    def open_layout(self, l):
        self.engine.open_saved(l)
        self.close()  # the group is what you wanted to see; this window's process ends

    def card_menu(self, l, gpos):
        is_open = self.engine.is_open(l.name)
        m = QMenu(self)
        m.addAction("Bring to front" if is_open else "Open").triggered.connect(lambda: self.open_layout(l))
        m.addAction(QIcon.fromTheme("document-edit"), "Edit").triggered.connect(lambda: self.edit(l))
        if is_open:
            m.addAction(QIcon.fromTheme("window-close"), "Close group").triggered.connect(
                lambda: self.engine.close_named(l.name))
        m.addSeparator()

        def delete():
            if ask(self, f"Delete the layout “{l.name}”?"):
                st = Store.load()
                st.remove(l.name)
                try:
                    st.save()
                except OSError as e:
                    ask(self, f"Couldn't save: {e}", question=False)
                self.refresh()

        m.addAction(QIcon.fromTheme("edit-delete"), "Delete").triggered.connect(delete)
        m.popup(gpos)


# ---------------------------------------------------------------- empty cell


class CellPicker(QDialog):
    """What goes into an empty cell of an open group (clicked "+")."""

    def __init__(self, client, gid: int, cell: int):
        super().__init__()
        self.client, self.gid, self.cell = client, gid, cell
        self.setWindowTitle("Fill this cell – ZTile")
        self.setWindowIcon(icons.app_icon())
        self.resize(460, 420)
        lay = QVBoxLayout(self)
        self.list = QListWidget()
        self.list.setIconSize(QSize(24, 24))
        lay.addWidget(self.list)
        row = QHBoxLayout()
        choose = QPushButton(QIcon.fromTheme("search"), "Choose an app to start…")
        choose.clicked.connect(self._choose)
        row.addWidget(choose)
        row.addStretch()
        bb = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        bb.accepted.connect(self._accept)
        bb.rejected.connect(self.reject)
        row.addWidget(bb)
        lay.addLayout(row)
        self.list.itemDoubleClicked.connect(lambda *_: self._accept())
        idx = apps.index()
        info = client.cell(gid, cell)
        spec = info.get("spec") if isinstance(info.get("spec"), dict) else None
        if spec and spec.get("app"):
            it = QListWidgetItem(idx.icon(spec["app"]), f"Start {info.get('name') or idx.name(spec['app'])}")
            it.setData(Qt.ItemDataRole.UserRole, ("launch", spec["app"]))
            self.list.addItem(it)
        head = QListWidgetItem("Put an open window here:")
        head.setFlags(Qt.ItemFlag.NoItemFlags)
        self.list.addItem(head)
        self.loading = QListWidgetItem("  …")
        self.loading.setFlags(Qt.ItemFlag.NoItemFlags)
        self.list.addItem(self.loading)
        client.request_windows(self._windows)
        self.list.setCurrentRow(0)

    def _windows(self, wins):
        idx = apps.index()
        self.list.takeItem(self.list.row(self.loading))
        free = [w for w in wins if w.get("group") is None]
        for w in free:
            it = QListWidgetItem(idx.icon(w.get("app") or ""), f"{short(w.get('caption') or '', 60)}  –  {idx.name(w.get('app') or '')}")
            it.setData(Qt.ItemDataRole.UserRole, ("fill", w.get("id")))
            self.list.addItem(it)
        if not free:
            none = QListWidgetItem("  (no other windows open)")
            none.setFlags(Qt.ItemFlag.NoItemFlags)
            self.list.addItem(none)

    def _accept(self):
        it = self.list.currentItem()
        what = it.data(Qt.ItemDataRole.UserRole) if it else None
        if not what:
            return
        kind, value = what
        if kind == "launch":
            self.client.launch(self.gid, self.cell, value)
        elif kind == "fill" and isinstance(value, str):
            self.client.fill(self.gid, self.cell, value)
        self.accept()

    def _choose(self):
        a = AppPicker.pick(self)
        if a:
            self.client.launch(self.gid, self.cell, a.id)
            self.accept()
