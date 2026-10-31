//! Layouts as normalized rectangles. (Phase 2 replaces this with a split tree.)

use windows::Win32::Foundation::RECT;

#[derive(Clone, Copy, Debug)]
pub struct NRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const fn r(x: f32, y: f32, w: f32, h: f32) -> NRect {
    NRect { x, y, w, h }
}

pub fn preset(name: &str) -> Option<Vec<NRect>> {
    const T: f32 = 1.0 / 3.0;
    Some(match name {
        "full" => vec![r(0., 0., 1., 1.)],
        "2v" => vec![r(0., 0., 0.5, 1.), r(0.5, 0., 0.5, 1.)],
        "2h" => vec![r(0., 0., 1., 0.5), r(0., 0.5, 1., 0.5)],
        "3v" => vec![r(0., 0., T, 1.), r(T, 0., T, 1.), r(2. * T, 0., T, 1.)],
        "3h" => vec![r(0., 0., 1., T), r(0., T, 1., T), r(0., 2. * T, 1., T)],
        "2x2" => vec![r(0., 0., 0.5, 0.5), r(0.5, 0., 0.5, 0.5), r(0., 0.5, 0.5, 0.5), r(0.5, 0.5, 0.5, 0.5)],
        "1+2" => vec![r(0., 0., 0.5, 1.), r(0.5, 0., 0.5, 0.5), r(0.5, 0.5, 0.5, 0.5)],
        "1+3" => vec![r(0., 0., 0.5, 1.), r(0.5, 0., 0.5, T), r(0.5, T, 0.5, T), r(0.5, 2. * T, 0.5, T)],
        "2+1" => vec![r(0., 0., 0.5, 0.5), r(0., 0.5, 0.5, 0.5), r(0.5, 0., 0.5, 1.)],
        _ => return None,
    })
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
    let eps = 0.001;
    cells
        .iter()
        .map(|c| {
            let x0 = area.left + (c.x * w).round() as i32;
            let y0 = area.top + (c.y * h).round() as i32;
            let x1 = area.left + ((c.x + c.w) * w).round() as i32;
            let y1 = area.top + ((c.y + c.h) * h).round() as i32;
            let edge = |at_edge: bool| if at_edge { gap } else { gap / 2 };
            RECT {
                left: x0 + edge(c.x < eps),
                top: y0 + edge(c.y < eps),
                right: x1 - edge(c.x + c.w > 1. - eps),
                bottom: y1 - edge(c.y + c.h > 1. - eps),
            }
        })
        .collect()
}
