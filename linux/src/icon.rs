//! Pixel drawing: ZTile's icon, a group's composite taskbar icon, the "+" glyph.

use crate::apps::Image;
use ztile_core::layout::NRect;

/// Straight-alpha RGBA canvas.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, px: vec![0; (w * h * 4) as usize] }
    }

    /// Fill a rect (float edges, anti-aliased by coverage) with rounded corners.
    pub fn round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, rgba: [u8; 4]) {
        let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        let (ys, ye) = (y0.floor().max(0.0) as u32, (y1.ceil() as u32).min(self.h));
        let (xs, xe) = (x0.floor().max(0.0) as u32, (x1.ceil() as u32).min(self.w));
        for y in ys..ye {
            for x in xs..xe {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let cx = fx.clamp(x0 + r, x1 - r);
                let cy = fy.clamp(y0 + r, y1 - r);
                // distance from the corner circle's centre: 0 outside the corner areas
                let dist = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let corner = if dist == 0.0 { 1.0 } else { (0.5 - (dist - r)).clamp(0.0, 1.0) };
                // coverage of the pixel by the straight edges
                let edge = (fx - x0).min(x1 - fx).min(fy - y0).min(y1 - fy) + 0.5;
                let cov = corner * edge.clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, rgba, cov);
                }
            }
        }
    }

    fn blend(&mut self, x: u32, y: u32, c: [u8; 4], cov: f32) {
        let i = ((y * self.w + x) * 4) as usize;
        let sa = c[3] as f32 / 255.0 * cov;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            let v = (c[k] as f32 * sa + self.px[i + k] as f32 * da * (1.0 - sa)) / oa;
            self.px[i + k] = v.round() as u8;
        }
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }

    /// Draw an image scaled into a square (box filter when shrinking).
    pub fn image(&mut self, img: &Image, x0: f32, y0: f32, size: f32) {
        let n = size.round().max(1.0) as u32;
        let (ox, oy) = (x0.round() as i64, y0.round() as i64);
        for ty in 0..n {
            for tx in 0..n {
                let (dx, dy) = (ox + tx as i64, oy + ty as i64);
                if dx < 0 || dy < 0 || dx >= self.w as i64 || dy >= self.h as i64 {
                    continue;
                }
                // source area covered by this target pixel
                let sx0 = tx as f32 * img.w as f32 / n as f32;
                let sx1 = (tx + 1) as f32 * img.w as f32 / n as f32;
                let sy0 = ty as f32 * img.h as f32 / n as f32;
                let sy1 = (ty + 1) as f32 * img.h as f32 / n as f32;
                let (mut acc, mut cnt) = ([0f32; 4], 0f32);
                let mut sy = sy0.floor() as u32;
                while (sy as f32) < sy1.max(sy0 + 1.0) && sy < img.h {
                    let mut sx = sx0.floor() as u32;
                    while (sx as f32) < sx1.max(sx0 + 1.0) && sx < img.w {
                        let i = ((sy * img.w + sx) * 4) as usize;
                        let a = img.px[i + 3] as f32;
                        for k in 0..3 {
                            acc[k] += img.px[i + k] as f32 * a;
                        }
                        acc[3] += a;
                        cnt += 1.0;
                        sx += 1;
                    }
                    sy += 1;
                }
                if cnt == 0.0 || acc[3] == 0.0 {
                    continue;
                }
                let a = acc[3] / cnt;
                let c = [(acc[0] / acc[3]) as u8, (acc[1] / acc[3]) as u8, (acc[2] / acc[3]) as u8, 255];
                self.blend(dx as u32, dy as u32, c, a / 255.0);
            }
        }
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, self.w, self.h);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            if let Ok(mut w) = e.write_header() {
                let _ = w.write_image_data(&self.px);
            }
        }
        out
    }

    /// Premultiplied ARGB32 in native byte order (wl_shm ARGB8888).
    pub fn to_argb_premul(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len());
        for p in self.px.chunks_exact(4) {
            let a = p[3] as u32;
            let pm = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            out.extend_from_slice(&u32::from_be_bytes([p[3], pm(p[0]), pm(p[1]), pm(p[2])]).to_ne_bytes());
        }
        out
    }

    /// Straight ARGB32 in network byte order (StatusNotifierItem IconPixmap).
    pub fn to_argb_network(&self) -> Vec<u8> {
        self.px.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect()
    }
}

const ACCENT: [u8; 4] = [0x4c, 0x9a, 0xff, 255];
const ACCENT_2: [u8; 4] = [0x7a, 0xb6, 0xff, 255];

/// Four rounded accent tiles in a 2x2 grid (same as the Windows version).
pub fn app_icon(size: u32) -> Canvas {
    let mut c = Canvas::new(size, size);
    let s = size as f32;
    let (m, gap) = ((s / 16.0).max(1.0), (s / 10.0).max(1.0));
    let tile = (s - 2.0 * m - gap) / 2.0;
    for (t, col) in [ACCENT, ACCENT_2, ACCENT_2, ACCENT].into_iter().enumerate() {
        let x = m + (t % 2) as f32 * (tile + gap);
        let y = m + (t / 2) as f32 * (tile + gap);
        c.round_rect(x, y, x + tile, y + tile, tile / 4.0, col);
    }
    c
}

const BG: [u8; 4] = [0x1c, 0x1c, 0x1c, 255];
const CELL: [u8; 4] = [0x3a, 0x3a, 0x3a, 255];
const EMPTY: [u8; 4] = [0x26, 0x26, 0x26, 255];

/// Dark rounded tile, one lighter slot per cell, each with its app's icon.
pub fn composite(cells: &[NRect], icons: &[Option<Image>], size: u32) -> Canvas {
    let mut c = Canvas::new(size, size);
    let s = size as f32;
    c.round_rect(0.0, 0.0, s, s, s / 6.0, BG);
    let gap = (s / 16.0).max(1.0);
    for (i, r) in cells.iter().enumerate() {
        let (x0, y0) = (r.x0 * (s - gap) + gap, r.y0 * (s - gap) + gap);
        let (x1, y1) = (r.x1 * (s - gap), r.y1 * (s - gap));
        let icon = icons.get(i).and_then(|x| x.as_ref());
        c.round_rect(x0, y0, x1, y1, 0.0, if icon.is_some() { CELL } else { EMPTY });
        if let Some(img) = icon {
            let side = (x1 - x0).min(y1 - y0);
            let sz = (side - side / 4.0).max(4.0);
            c.image(img, x0 + (x1 - x0 - sz) / 2.0, y0 + (y1 - y0 - sz) / 2.0, sz);
        }
    }
    c
}

/// The "+" of an empty cell: grey cross on transparent.
pub fn plus_glyph(size: u32) -> Canvas {
    let mut c = Canvas::new(size, size);
    let s = size as f32;
    let t = (s / 14.0).max(1.0);
    let col = [0x80, 0x80, 0x80, 255];
    c.round_rect(s * 0.2, s / 2.0 - t / 2.0, s * 0.8, s / 2.0 + t / 2.0, t / 2.0, col);
    c.round_rect(s / 2.0 - t / 2.0, s * 0.2, s / 2.0 + t / 2.0, s * 0.8, t / 2.0, col);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztile_core::layout;

    #[test]
    fn composite_draws_and_encodes() {
        let red = Image { w: 2, h: 2, px: [255, 0, 0, 255].repeat(4) };
        let c = composite(&layout::preset("2v").unwrap().cells(), &[Some(red), None], 64);
        let at = |x: u32, y: u32| &c.px[((y * 64 + x) * 4) as usize..][..4];
        assert_eq!(at(0, 0)[3], 0); // rounded corner: transparent
        assert_eq!(at(16, 32)[..3], [255, 0, 0]); // icon in the left cell
        assert_eq!(at(48, 32)[..3], EMPTY[..3]); // empty right cell
        assert!(c.png().starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
