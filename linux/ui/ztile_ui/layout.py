"""Layouts as a split tree: a node is either a cell, or a row/column of weighted
children. Cells are numbered in depth-first (reading) order.

Same model and JSON form as the Windows version:
  "cell"  |  {"dir": "cols" | "rows", "sizes": [..], "children": [..]}
Text form (CLI): c | cols(...) | rows(...), children may carry a weight:
  cols(60:c, 40:rows(c, c))
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

CELL = "cell"
MAX_CELLS = 16
MAX_DEPTH = 8


@dataclass
class Split:
    dir: str  # "cols": side by side, "rows": stacked
    sizes: list[float]
    children: list  # of Split | CELL


@dataclass
class Divider:
    """Boundary between children k and k+1 of the split at `path`."""

    path: list[int]
    k: int
    dir: str
    pos: float
    parent: tuple[float, float, float, float] = field(default=(0.0, 0.0, 1.0, 1.0))


def clean_size(v) -> float:
    try:
        v = float(v)
    except (TypeError, ValueError):
        return 1.0
    return min(max(v, 0.01), 1000.0) if math.isfinite(v) else 1.0


def even(d: str, children: list) -> Split:
    return Split(d, [1.0] * len(children), children)


def cell_count(n) -> int:
    return 1 if n == CELL else sum(cell_count(c) for c in n.children)


def child_rects(r, d, sizes):
    """Children's rects; shared edges computed by the same expression on both sides."""
    x0, y0, x1, y1 = r
    total = max(sum(sizes), 1e-9)
    out, acc, n = [], 0.0, len(sizes)
    for i, s in enumerate(sizes):
        f0 = acc / total
        acc += s
        last = i + 1 == n
        if d == "cols":
            w = x1 - x0
            out.append((x0 if i == 0 else x0 + w * f0, y0, x1 if last else x0 + w * (acc / total), y1))
        else:
            h = y1 - y0
            out.append((x0, y0 if i == 0 else y0 + h * f0, x1, y1 if last else y0 + h * (acc / total)))
    return out


def cells(n, r=(0.0, 0.0, 1.0, 1.0)) -> list[tuple[float, float, float, float]]:
    """Normalized cell rects (x0, y0, x1, y1) in reading order."""
    if n == CELL:
        return [r]
    out = []
    for sub, c in zip(child_rects(r, n.dir, n.sizes), n.children):
        out += cells(c, sub)
    return out


def _cell_path(n, idx):
    counter = [idx]

    def walk(node, path):
        if node == CELL:
            if counter[0] == 0:
                return path
            counter[0] -= 1
            return None
        for i, c in enumerate(node.children):
            p = walk(c, path + [i])
            if p is not None:
                return p
        return None

    return walk(n, [])


def _node(n, path):
    for i in path:
        if n == CELL:
            return None
        n = n.children[i]
    return n


def split_cell(root, idx: int, d: str):
    """Split cell idx in two along d; the new cell is idx+1. Returns the new root or None."""
    if cell_count(root) >= MAX_CELLS:
        return None
    path = _cell_path(root, idx)
    if path is None:
        return None
    if path:
        parent = _node(root, path[:-1])
        last = path[-1]
        if parent.dir == d:  # same direction: just add a sibling
            half = parent.sizes[last] / 2
            parent.sizes[last] = half
            parent.sizes.insert(last + 1, half)
            parent.children.insert(last + 1, CELL)
            return root
    if len(path) >= MAX_DEPTH:
        return None
    if not path:
        return even(d, [CELL, CELL])
    parent = _node(root, path[:-1])
    parent.children[path[-1]] = even(d, [CELL, CELL])
    return root


def remove_cell(root, idx: int):
    """Remove cell idx; neighbours grow into its space. Returns the new root or None."""
    path = _cell_path(root, idx)
    if not path:
        return None
    parent_path, last = path[:-1], path[-1]
    parent = _node(root, parent_path)
    del parent.sizes[last]
    del parent.children[last]
    if len(parent.children) == 1:  # a split with one child is that child
        only = parent.children[0]
        if not parent_path:
            return only
        _node(root, parent_path[:-1]).children[parent_path[-1]] = only
    return root


def dividers(root) -> list[Divider]:
    out = []

    def walk(n, r, path):
        if n == CELL:
            return
        subs = child_rects(r, n.dir, n.sizes)
        for k in range(len(subs) - 1):
            pos = subs[k][2] if n.dir == "cols" else subs[k][3]
            out.append(Divider(list(path), k, n.dir, pos, r))
        for i, (sub, c) in enumerate(zip(subs, n.children)):
            walk(c, sub, path + [i])

    walk(root, (0.0, 0.0, 1.0, 1.0), [])
    return out


def move_divider(root, d: Divider, pos: float):
    """Move a divider to normalized position pos; each side keeps at least 5%."""
    n = _node(root, d.path)
    if n is None or n == CELL or d.k + 1 >= len(n.sizes):
        return
    a, b = (d.parent[0], d.parent[2]) if d.dir == "cols" else (d.parent[1], d.parent[3])
    if b - a <= 1e-6:
        return
    total = sum(n.sizes)
    before = sum(n.sizes[: d.k])
    pair = n.sizes[d.k] + n.sizes[d.k + 1]
    mn = total * 0.05
    want = ((pos - a) / (b - a)) * total - before
    k_size = min(max(want, min(mn, pair / 2)), max(pair - mn, pair / 2))
    n.sizes[d.k] = k_size
    n.sizes[d.k + 1] = pair - k_size


def to_json(n):
    if n == CELL:
        return CELL
    return {"dir": n.dir, "sizes": [round(s, 2) for s in n.sizes], "children": [to_json(c) for c in n.children]}


def from_json(j, depth=0):
    """None for anything malformed, too deep or with too many cells."""
    n = _from_json(j, depth)
    return n if n is not None and cell_count(n) <= MAX_CELLS else None


def _from_json(j, depth):
    if j == CELL:
        return CELL
    if depth >= MAX_DEPTH or not isinstance(j, dict) or j.get("dir") not in ("cols", "rows"):
        return None
    kids = j.get("children")
    if not isinstance(kids, list) or not kids or len(kids) > MAX_CELLS:
        return None
    children = [_from_json(c, depth + 1) for c in kids]
    if any(c is None for c in children):
        return None
    sizes = j.get("sizes")
    sizes = [clean_size(v) for v in sizes] if isinstance(sizes, list) else []
    sizes = (sizes + [1.0] * len(children))[: len(children)]
    return Split(j["dir"], sizes, children)


def to_spec(n) -> str:
    if n == CELL:
        return "c"
    uneven = any(abs(a - b) > 1e-4 for a, b in zip(n.sizes, n.sizes[1:]))
    total = sum(n.sizes)
    parts = [
        (f"{round(s / total * 100, 2):g}:" if uneven else "") + to_spec(c) for s, c in zip(n.sizes, n.children)
    ]
    return f"{n.dir}({', '.join(parts)})"


def parse_spec(s: str):
    if len(s) > 1024:
        return None
    pos = [0]

    def ws():
        while pos[0] < len(s) and s[pos[0]].isspace():
            pos[0] += 1

    def eat(lit):
        ws()
        if s.startswith(lit, pos[0]):
            pos[0] += len(lit)
            return True
        return False

    def node(depth):
        if eat("cols"):
            d = "cols"
        elif eat("rows"):
            d = "rows"
        elif eat("c"):
            return CELL
        else:
            return None
        if not eat("(") or depth >= MAX_DEPTH:
            return None
        sizes, children = [], []
        while True:
            ws()
            start = pos[0]
            while pos[0] < len(s) and (s[pos[0]].isdigit() or s[pos[0]] == "."):
                pos[0] += 1
            weight = 1.0
            if pos[0] > start and eat(":"):
                try:
                    weight = float(s[start : pos[0] - 1].strip())
                except ValueError:
                    return None
            else:
                pos[0] = start
            sizes.append(clean_size(weight))
            c = node(depth + 1)
            if c is None or len(children) >= MAX_CELLS:
                return None
            children.append(c)
            if eat(","):
                continue
            if eat(")"):
                return Split(d, sizes, children)
            return None

    n = node(0)
    ws()
    return n if n is not None and pos[0] == len(s) and cell_count(n) <= MAX_CELLS else None


PRESETS = ["full", "2v", "2h", "3v", "3h", "2x2", "1+2", "1+3", "2+1"]


def preset(name: str):
    C = CELL
    return {
        "full": lambda: C,
        "2v": lambda: even("cols", [C, C]),
        "2h": lambda: even("rows", [C, C]),
        "3v": lambda: even("cols", [C, C, C]),
        "3h": lambda: even("rows", [C, C, C]),
        "2x2": lambda: even("rows", [even("cols", [C, C]), even("cols", [C, C])]),
        "1+2": lambda: even("cols", [C, even("rows", [C, C])]),
        "1+3": lambda: even("cols", [C, even("rows", [C, C, C])]),
        "2+1": lambda: even("cols", [even("rows", [C, C]), C]),
    }.get(name, lambda: None)()


def from_name_or_spec(s: str):
    return preset(s) or parse_spec(s)


def to_pixels(cs, area, gap):
    """Map normalized cells into area (x, y, w, h) with gap px between/around cells."""
    ax, ay, aw, ah = area

    def edge(at_border):
        return gap if at_border else gap / 2

    out = []
    for x0, y0, x1, y1 in cs:
        l = round(ax + x0 * aw + edge(x0 <= 0))
        t = round(ay + y0 * ah + edge(y0 <= 0))
        r = round(ax + x1 * aw - edge(x1 >= 1))
        b = round(ay + y1 * ah - edge(y1 >= 1))
        out.append((l, t, max(1, r - l), max(1, b - t)))
    return out
