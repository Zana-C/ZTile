//! Layouts as a split tree: a node is either a cell, or a row/column of
//! weighted children. Cells are numbered in depth-first (reading) order.
//!
//! Text form (CLI, debugging):  c | cols(...) | rows(...)
//! children may carry a weight:  cols(60:c, 40:rows(c, c))

use crate::json::Json;
use windows::Win32::Foundation::RECT;

/// Normalized rectangle by its edges, 0..1. Edges (not x/w) so that neighbouring
/// cells share bit-identical edge values and round to the same pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// children side by side, left to right
    Cols,
    /// children stacked, top to bottom
    Rows,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Cell,
    Split { dir: Dir, sizes: Vec<f32>, children: Vec<Node> },
}

use Node::Cell;

fn even(dir: Dir, children: Vec<Node>) -> Node {
    Node::Split { dir, sizes: vec![1.0; children.len()], children }
}

impl Node {
    pub fn cell_count(&self) -> usize {
        match self {
            Cell => 1,
            Node::Split { children, .. } => children.iter().map(Node::cell_count).sum(),
        }
    }

    /// Cell rectangles in reading order, normalized to 0..1.
    pub fn cells(&self) -> Vec<NRect> {
        let mut out = Vec::new();
        self.collect(NRect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }, &mut out);
        out
    }

    fn collect(&self, r: NRect, out: &mut Vec<NRect>) {
        match self {
            Cell => out.push(r),
            Node::Split { dir, sizes, children } => {
                for (sub, child) in child_rects(r, *dir, sizes).into_iter().zip(children) {
                    child.collect(sub, out);
                }
            }
        }
    }

    /// Child indices leading to cell number `idx` (empty path = the root is that cell).
    fn cell_path(&self, idx: usize) -> Option<Vec<usize>> {
        fn walk(n: &Node, idx: &mut usize, path: &mut Vec<usize>) -> bool {
            match n {
                Cell => {
                    if *idx == 0 {
                        return true;
                    }
                    *idx -= 1;
                    false
                }
                Node::Split { children, .. } => {
                    for (i, c) in children.iter().enumerate() {
                        path.push(i);
                        if walk(c, idx, path) {
                            return true;
                        }
                        path.pop();
                    }
                    false
                }
            }
        }
        let (mut i, mut path) = (idx, Vec::new());
        walk(self, &mut i, &mut path).then_some(path)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut n = self;
        for &i in path {
            match n {
                Node::Split { children, .. } => n = children.get_mut(i)?,
                Cell => return None,
            }
        }
        Some(n)
    }

    /// Split cell `idx` in two along `dir`; the new cell becomes number `idx + 1`.
    pub fn split_cell(&mut self, idx: usize, dir: Dir) {
        let Some(path) = self.cell_path(idx) else { return };
        if let Some((&last, parent_path)) = path.split_last() {
            if let Some(Node::Split { dir: pd, sizes, children }) = self.node_mut(parent_path) {
                if *pd == dir {
                    // same direction as the parent: just add a sibling
                    let half = sizes[last] / 2.0;
                    sizes[last] = half;
                    sizes.insert(last + 1, half);
                    children.insert(last + 1, Cell);
                    return;
                }
            }
        }
        if let Some(n) = self.node_mut(&path) {
            *n = even(dir, vec![Cell, Cell]);
        }
    }

    /// Remove cell `idx`; its neighbours grow into the space. The last cell can't go.
    pub fn remove_cell(&mut self, idx: usize) -> bool {
        let Some(path) = self.cell_path(idx) else { return false };
        let Some((&last, parent_path)) = path.split_last() else { return false };
        let Some(parent) = self.node_mut(parent_path) else { return false };
        if let Node::Split { sizes, children, .. } = parent {
            sizes.remove(last);
            children.remove(last);
            if children.len() == 1 {
                let only = children.pop().unwrap();
                *parent = only; // a split with one child is just that child
            }
            return true;
        }
        false
    }

    /// Draggable boundaries between neighbouring children, for the editor.
    pub fn dividers(&self) -> Vec<Divider> {
        fn walk(n: &Node, r: NRect, path: &mut Vec<usize>, out: &mut Vec<Divider>) {
            if let Node::Split { dir, sizes, children } = n {
                let subs = child_rects(r, *dir, sizes);
                for k in 0..subs.len() - 1 {
                    let pos = if *dir == Dir::Cols { subs[k].x1 } else { subs[k].y1 };
                    out.push(Divider { path: path.clone(), k, dir: *dir, pos, parent: r });
                }
                for (i, (sub, c)) in subs.into_iter().zip(children).enumerate() {
                    path.push(i);
                    walk(c, sub, path, out);
                    path.pop();
                }
            }
        }
        let mut out = Vec::new();
        walk(self, NRect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }, &mut Vec::new(), &mut out);
        out
    }

    /// Move a divider to normalized position `pos` (same axis as the divider).
    /// Only the two children next to it change; each keeps at least 5%.
    pub fn move_divider(&mut self, d: &Divider, pos: f32) {
        let Some(Node::Split { sizes, .. }) = self.node_mut(&d.path) else { return };
        let (a, b) = if d.dir == Dir::Cols { (d.parent.x0, d.parent.x1) } else { (d.parent.y0, d.parent.y1) };
        if b - a <= f32::EPSILON || d.k + 1 >= sizes.len() {
            return;
        }
        let total: f32 = sizes.iter().sum();
        let before: f32 = sizes[..d.k].iter().sum();
        let pair = sizes[d.k] + sizes[d.k + 1];
        let min = total * 0.05;
        let want = ((pos - a) / (b - a)) * total - before;
        let k_size = want.clamp(min.min(pair / 2.0), (pair - min).max(pair / 2.0));
        sizes[d.k] = k_size;
        sizes[d.k + 1] = pair - k_size;
    }

    pub fn to_json(&self) -> Json {
        match self {
            Cell => Json::str("cell"),
            Node::Split { dir, sizes, children } => Json::obj(vec![
                ("dir", Json::str(if *dir == Dir::Cols { "cols" } else { "rows" })),
                ("sizes", Json::Arr(sizes.iter().map(|s| Json::Num(round2(*s) as f64)).collect())),
                ("children", Json::Arr(children.iter().map(Node::to_json).collect())),
            ]),
        }
    }

    pub fn from_json(j: &Json) -> Option<Node> {
        if j.as_str() == Some("cell") {
            return Some(Cell);
        }
        let dir = match j.get("dir")?.as_str()? {
            "cols" => Dir::Cols,
            "rows" => Dir::Rows,
            _ => return None,
        };
        let children: Vec<Node> = j.get("children")?.as_arr()?.iter().map(Node::from_json).collect::<Option<_>>()?;
        let mut sizes: Vec<f32> = match j.get("sizes").and_then(Json::as_arr) {
            Some(a) => a.iter().map(|v| v.as_f64().unwrap_or(1.0).max(0.01) as f32).collect(),
            None => vec![1.0; children.len()],
        };
        sizes.resize(children.len(), 1.0);
        if children.is_empty() {
            return None;
        }
        Some(Node::Split { dir, sizes, children })
    }

    /// Text form, e.g. `cols(c, rows(c, c))`; weights only when uneven.
    pub fn to_spec(&self) -> String {
        match self {
            Cell => "c".into(),
            Node::Split { dir, sizes, children } => {
                let uneven = sizes.windows(2).any(|w| (w[0] - w[1]).abs() > 1e-4);
                let total: f32 = sizes.iter().sum();
                let parts: Vec<String> = children
                    .iter()
                    .zip(sizes)
                    .map(|(c, s)| {
                        if uneven { format!("{}:{}", round2(s / total * 100.0), c.to_spec()) } else { c.to_spec() }
                    })
                    .collect();
                format!("{}({})", if *dir == Dir::Cols { "cols" } else { "rows" }, parts.join(", "))
            }
        }
    }

    pub fn parse_spec(s: &str) -> Option<Node> {
        let mut p = Spec { s: s.as_bytes(), i: 0 };
        let n = p.node()?;
        p.ws();
        (p.i == p.s.len()).then_some(n)
    }
}

/// A boundary between children `k` and `k + 1` of the split at `path`.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub path: Vec<usize>,
    pub k: usize,
    /// Cols: vertical line at x = pos; Rows: horizontal line at y = pos
    pub dir: Dir,
    pub pos: f32,
    /// the split's own rectangle (the line spans across it)
    pub parent: NRect,
}

/// Rectangles of a split's children. Shared boundaries are computed by the same
/// expression on both sides so they're bit-identical.
fn child_rects(r: NRect, dir: Dir, sizes: &[f32]) -> Vec<NRect> {
    let total: f32 = sizes.iter().sum::<f32>().max(f32::EPSILON);
    let n = sizes.len();
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(n);
    for (i, s) in sizes.iter().enumerate() {
        let f0 = acc / total;
        acc += s;
        let last = i + 1 == n; // last child ends exactly at the parent's edge
        out.push(match dir {
            Dir::Cols => {
                let w = r.x1 - r.x0;
                NRect {
                    x0: if i == 0 { r.x0 } else { r.x0 + w * f0 },
                    x1: if last { r.x1 } else { r.x0 + w * (acc / total) },
                    ..r
                }
            }
            Dir::Rows => {
                let h = r.y1 - r.y0;
                NRect {
                    y0: if i == 0 { r.y0 } else { r.y0 + h * f0 },
                    y1: if last { r.y1 } else { r.y0 + h * (acc / total) },
                    ..r
                }
            }
        });
    }
    out
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

struct Spec<'a> {
    s: &'a [u8],
    i: usize,
}

impl Spec<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn eat(&mut self, lit: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }
    fn node(&mut self) -> Option<Node> {
        let dir = if self.eat("cols") {
            Dir::Cols
        } else if self.eat("rows") {
            Dir::Rows
        } else if self.eat("c") {
            return Some(Cell);
        } else {
            return None;
        };
        if !self.eat("(") {
            return None;
        }
        let (mut sizes, mut children) = (Vec::new(), Vec::new());
        loop {
            self.ws();
            // optional "weight:"
            let start = self.i;
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
                self.i += 1;
            }
            let weight = if self.i > start && self.eat(":") {
                std::str::from_utf8(&self.s[start..self.i - 1]).ok()?.trim().parse::<f32>().ok()?
            } else {
                self.i = start;
                1.0
            };
            sizes.push(weight.max(0.01));
            children.push(self.node()?);
            if self.eat(",") {
                continue;
            }
            if self.eat(")") {
                break;
            }
            return None;
        }
        Some(Node::Split { dir, sizes, children })
    }
}

pub const PRESETS: &[&str] = &["full", "2v", "2h", "3v", "3h", "2x2", "1+2", "1+3", "2+1"];

pub fn preset(name: &str) -> Option<Node> {
    use Dir::*;
    Some(match name {
        "full" => Cell,
        "2v" => even(Cols, vec![Cell, Cell]),
        "2h" => even(Rows, vec![Cell, Cell]),
        "3v" => even(Cols, vec![Cell, Cell, Cell]),
        "3h" => even(Rows, vec![Cell, Cell, Cell]),
        "2x2" => even(Rows, vec![even(Cols, vec![Cell, Cell]), even(Cols, vec![Cell, Cell])]),
        "1+2" => even(Cols, vec![Cell, even(Rows, vec![Cell, Cell])]),
        "1+3" => even(Cols, vec![Cell, even(Rows, vec![Cell, Cell, Cell])]),
        "2+1" => even(Cols, vec![even(Rows, vec![Cell, Cell]), Cell]),
        _ => return None,
    })
}

/// Preset name or text spec.
pub fn from_name_or_spec(s: &str) -> Option<Node> {
    preset(s).or_else(|| Node::parse_spec(s))
}

pub fn default_for(count: usize) -> &'static str {
    match count {
        0 | 1 => "full",
        2 => "2v",
        3 => "3v",
        _ => "2x2",
    }
}

/// Map normalized cells into a pixel area with `gap` px between/around cells.
pub fn to_pixels(cells: &[NRect], area: RECT, gap: i32) -> Vec<RECT> {
    let w = (area.right - area.left) as f32;
    let h = (area.bottom - area.top) as f32;
    let px = |o: i32, size: f32, f: f32| o + (f * size).round() as i32;
    let edge = |at_border: bool| if at_border { gap } else { gap / 2 };
    cells
        .iter()
        .map(|c| RECT {
            left: px(area.left, w, c.x0) + edge(c.x0 <= 0.0),
            top: px(area.top, h, c.y0) + edge(c.y0 <= 0.0),
            right: px(area.left, w, c.x1) - edge(c.x1 >= 1.0),
            bottom: px(area.top, h, c.y1) - edge(c.y1 >= 1.0),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: RECT = RECT { left: 0, top: 0, right: 2560, bottom: 1540 };

    #[test]
    fn presets_have_expected_cells() {
        for (name, n) in [("full", 1), ("2v", 2), ("3h", 3), ("2x2", 4), ("1+3", 4), ("2+1", 3)] {
            assert_eq!(preset(name).unwrap().cell_count(), n, "{name}");
            assert_eq!(preset(name).unwrap().cells().len(), n, "{name}");
        }
    }

    #[test]
    fn grid_order_is_reading_order() {
        let c = preset("2x2").unwrap().cells();
        assert_eq!((c[0].x0, c[0].y0), (0.0, 0.0)); // top-left
        assert_eq!((c[1].x0, c[1].y0), (0.5, 0.0)); // top-right
        assert_eq!((c[2].x0, c[2].y0), (0.0, 0.5)); // bottom-left
        assert_eq!((c[3].x0, c[3].y0), (0.5, 0.5)); // bottom-right
    }

    #[test]
    fn thirds_tile_without_gaps_or_overlap() {
        let px = to_pixels(&preset("3v").unwrap().cells(), RECT { right: 1001, ..AREA }, 0);
        assert_eq!(px[0].left, 0);
        assert_eq!(px[0].right, px[1].left);
        assert_eq!(px[1].right, px[2].left);
        assert_eq!(px[2].right, 1001);
    }

    #[test]
    fn gaps() {
        let px = to_pixels(&preset("2v").unwrap().cells(), AREA, 8);
        assert_eq!((px[0].left, px[0].top, px[0].right, px[0].bottom), (8, 8, 1276, 1532));
        assert_eq!(px[1].left, 1284);
        assert_eq!(px[1].right, 2552);
    }

    #[test]
    fn weights() {
        let n = Node::parse_spec("cols(60:c, 40:rows(c, c))").unwrap();
        let px = to_pixels(&n.cells(), AREA, 0);
        assert_eq!(px[0].right, 1536);
        assert_eq!(px[1].left, 1536);
        assert_eq!(px[1].bottom, px[2].top);
        assert_eq!(n.to_spec(), "cols(60:c, 40:rows(c, c))");
    }

    #[test]
    fn split_and_remove() {
        let mut n = preset("2v").unwrap();
        n.split_cell(1, Dir::Rows); // right column -> two rows
        assert_eq!(n.to_spec(), "cols(c, rows(c, c))");
        n.split_cell(0, Dir::Cols); // same dir as root: becomes a sibling
        assert_eq!(n.to_spec(), "cols(25:c, 25:c, 50:rows(c, c))");
        assert!(n.remove_cell(3));
        assert_eq!(n.cell_count(), 3);
        assert_eq!(n.to_spec(), "cols(25:c, 25:c, 50:c)");
        let mut one = Node::Cell;
        assert!(!one.remove_cell(0));
        one.split_cell(0, Dir::Rows);
        assert_eq!(one.to_spec(), "rows(c, c)");
    }

    #[test]
    fn move_divider_clamps() {
        let mut n = preset("3v").unwrap();
        let d = n.dividers()[0].clone();
        n.move_divider(&d, 0.5);
        let c = n.cells();
        assert!((c[0].x1 - 0.5).abs() < 1e-4);
        assert!((c[1].x1 - 2.0 / 3.0).abs() < 1e-4); // third column untouched
        n.move_divider(&d, 0.99); // can't swallow the neighbour
        assert!(n.cells()[1].x1 - n.cells()[1].x0 >= 0.049);
    }

    #[test]
    fn nested_dividers() {
        let n = preset("1+2").unwrap();
        let ds = n.dividers();
        assert_eq!(ds.len(), 2);
        assert_eq!((ds[0].dir, ds[0].pos), (Dir::Cols, 0.5));
        assert_eq!((ds[1].dir, ds[1].pos, ds[1].parent.x0), (Dir::Rows, 0.5, 0.5));
    }

    #[test]
    fn spec_and_json_roundtrip() {
        for name in PRESETS {
            let n = preset(name).unwrap();
            assert_eq!(Node::parse_spec(&n.to_spec()).unwrap(), n, "{name}");
            let j = Json::parse(&n.to_json().pretty()).unwrap();
            assert_eq!(Node::from_json(&j).unwrap(), n, "{name}");
        }
        assert!(Node::parse_spec("cols(c,").is_none());
        assert!(Node::parse_spec("rows()").is_none());
    }
}
