//! The layout model is shared with the Linux version (`core/src/layout.rs`);
//! this adds the conversion to Win32 rectangles.

pub use ztile_core::layout::*;
use windows::Win32::Foundation::RECT;

/// Map normalized cells into a pixel area with `gap` px between/around cells.
pub fn to_pixels(cells: &[NRect], area: RECT, gap: i32) -> Vec<RECT> {
    let a = Rect { left: area.left, top: area.top, right: area.right, bottom: area.bottom };
    ztile_core::layout::to_pixels(cells, a, gap)
        .into_iter()
        .map(|r| RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
        .collect()
}
