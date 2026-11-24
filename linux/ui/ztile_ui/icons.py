"""ZTile's own icon and the composite "layout" icon of a group's taskbar button."""

from __future__ import annotations

from PyQt6.QtCore import QRectF, Qt
from PyQt6.QtGui import QColor, QIcon, QImage, QPainter, QPainterPath, QPixmap

ACCENT = QColor("#4c9aff")
ACCENT_2 = QColor("#7ab6ff")


def app_icon_image(size: int) -> QImage:
    """Four rounded accent tiles in a 2x2 grid."""
    img = QImage(size, size, QImage.Format.Format_ARGB32_Premultiplied)
    img.fill(Qt.GlobalColor.transparent)
    p = QPainter(img)
    p.setRenderHint(QPainter.RenderHint.Antialiasing)
    p.setPen(Qt.PenStyle.NoPen)
    m, gap = max(1.0, size / 16), max(1.0, size / 10)
    tile = (size - 2 * m - gap) / 2
    for t, col in enumerate([ACCENT, ACCENT_2, ACCENT_2, ACCENT]):
        x = m + (t % 2) * (tile + gap)
        y = m + (t // 2) * (tile + gap)
        p.setBrush(col)
        p.drawRoundedRect(QRectF(x, y, tile, tile), tile / 4, tile / 4)
    p.end()
    return img


def app_icon() -> QIcon:
    ic = QIcon.fromTheme("io.github.ZanaC.ZTile")
    if not ic.isNull():
        return ic
    ic = QIcon()
    for s in (16, 22, 24, 32, 48, 64, 128):
        ic.addPixmap(QPixmap.fromImage(app_icon_image(s)))
    return ic


BG = QColor("#1c1c1c")
CELL = QColor("#3a3a3a")
EMPTY = QColor("#262626")


def composite(ncells, icons, size: int = 128) -> QImage:
    """Dark rounded tile, one lighter slot per cell, each showing its app icon."""
    img = QImage(size, size, QImage.Format.Format_ARGB32_Premultiplied)
    img.fill(Qt.GlobalColor.transparent)
    p = QPainter(img)
    p.setRenderHint(QPainter.RenderHint.Antialiasing)
    p.setRenderHint(QPainter.RenderHint.SmoothPixmapTransform)
    clip = QPainterPath()
    clip.addRoundedRect(QRectF(0, 0, size, size), size / 6, size / 6)
    p.fillPath(clip, BG)
    p.setClipPath(clip)
    s = float(size)
    gap = max(1.0, s / 16)
    for i, (x0, y0, x1, y1) in enumerate(ncells):
        r = QRectF(x0 * (s - gap) + gap, y0 * (s - gap) + gap, 0, 0)
        r.setRight(x1 * (s - gap))
        r.setBottom(y1 * (s - gap))
        ic = icons[i] if i < len(icons) else None
        p.fillRect(r, CELL if ic is not None else EMPTY)
        if ic is not None and not ic.isNull():
            side = min(r.width(), r.height())
            pad = side / 8
            sz = max(4, int(side - 2 * pad))
            pm = ic.pixmap(sz, sz)
            p.drawPixmap(
                QRectF(r.center().x() - sz / 2, r.center().y() - sz / 2, sz, sz).toRect(), pm
            )
    p.end()
    return img
