//! App icon lookup + composite "layout" icon for the taskbar.

use crate::layout::NRect;
use crate::win;
use std::ffi::c_void;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

/// Best-effort icon of a window (not owned by us, don't destroy).
pub fn window_icon(h: HWND) -> HICON {
    unsafe {
        // ICON_BIG = 1, ICON_SMALL2 = 2, ICON_SMALL = 0
        for kind in [1usize, 2, 0] {
            let mut res = 0usize;
            let ok = SendMessageTimeoutW(
                h,
                WM_GETICON,
                WPARAM(kind),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                100,
                Some(&mut res),
            );
            if ok.0 != 0 && res != 0 {
                return HICON(res as *mut c_void);
            }
        }
        for idx in [GCLP_HICON, GCLP_HICONSM] {
            let c = GetClassLongPtrW(h, idx);
            if c != 0 {
                return HICON(c as *mut c_void);
            }
        }
        let path = win::wide(&win::exe_path(h));
        let mut large = HICON::default();
        ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&mut large), None, 1);
        if !large.is_invalid() {
            return large;
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

const BG: u32 = 0xFF1C1C1C;
const CELL: u32 = 0xFF3A3A3A;
const EMPTY: u32 = 0xFF262626;

/// Draw a `size`Ã—`size` icon: dark rounded tile, one lighter cell per layout slot,
/// each slot showing its app icon (or nothing if empty).
pub fn composite(cells: &[NRect], icons: &[Option<HICON>], size: i32) -> HICON {
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return HICON::default();
        };
        let old = SelectObject(mem, dib.into());
        let n = size as usize;
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, n * n);
        px.fill(BG);

        let s = size as f32;
        let gap = (s / 16.0).max(1.0);
        let mut slots = Vec::new();
        for c in cells {
            let x0 = (c.x * (s - gap) + gap).round() as i32;
            let y0 = (c.y * (s - gap) + gap).round() as i32;
            let x1 = ((c.x + c.w) * (s - gap)).round() as i32;
            let y1 = ((c.y + c.h) * (s - gap)).round() as i32;
            slots.push((x0, y0, x1, y1));
        }
        for (i, &(x0, y0, x1, y1)) in slots.iter().enumerate() {
            let col = if icons.get(i).copied().flatten().is_some() { CELL } else { EMPTY };
            for y in y0.max(0)..y1.min(size) {
                for x in x0.max(0)..x1.min(size) {
                    px[y as usize * n + x as usize] = col;
                }
            }
        }
        let _ = GdiFlush();
        for (i, &(x0, y0, x1, y1)) in slots.iter().enumerate() {
            let Some(Some(ic)) = icons.get(i) else { continue };
            let pad = ((x1 - x0).min(y1 - y0) / 8).max(0);
            let sz = ((x1 - x0).min(y1 - y0) - 2 * pad).max(4);
            let ix = x0 + ((x1 - x0) - sz) / 2;
            let iy = y0 + ((y1 - y0) - sz) / 2;
            let _ = DrawIconEx(mem, ix, iy, *ic, sz, sz, 0, None, DI_NORMAL);
        }
        let _ = GdiFlush();

        // Force opaque, then cut rounded corners.
        let r = (s / 6.0).max(2.0);
        for y in 0..n {
            for x in 0..n {
                let p = &mut px[y * n + x];
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let cx = if fx < r { r } else if fx > s - r { s - r } else { fx };
                let cy = if fy < r { r } else if fy > s - r { s - r } else { fy };
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                *p = if d > r { 0 } else { *p | 0xFF00_0000 };
            }
        }

        SelectObject(mem, old);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        let mask = CreateBitmap(size, size, 1, 1, None);
        let ii = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: dib };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(dib.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
