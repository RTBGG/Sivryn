//! The mouse pointer in screen shares, drawn by Sivryn itself.
//!
//! libwebrtc's capturers only *prefer* an embedded cursor; its DXGI capturer
//! never draws one (Chrome adds it separately), and on the HDR path it is up
//! to Windows. So both paths capture without it, and this draws the current
//! pointer into every frame: its shape from Windows (rendered once per
//! cursor, on black and on white to get its transparency, so inverting
//! cursors work too) at its position over the shared screen or window.

use livekit::webrtc::prelude::{I420Buffer, VideoBuffer};

use crate::screen::{ScreenSource, SourceKind};

/// A cursor image with straight alpha and its hot spot.
#[derive(Clone)]
pub(crate) struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub hotspot: (i32, i32),
    pub rgba: Vec<[u8; 4]>,
}

/// Draws the pointer into frames of one source.
pub(crate) struct CursorOverlay {
    source: ScreenSource,
    /// The last cursor rendered, by its handle.
    cached: Option<(isize, CursorImage)>,
    /// Where and which pointer the last frame got (none while hidden).
    drawn: Option<(i32, i32, isize)>,
}

impl CursorOverlay {
    pub fn new(source: &ScreenSource) -> Self {
        Self { source: source.clone(), cached: None, drawn: None }
    }

    /// Where the pointer is now in a frame of this size, with its cursor.
    fn pointer(&self, width: u32, height: u32) -> Option<(i32, i32, isize)> {
        #[cfg(windows)]
        return imp::pointer_in(&self.source, width, height);
        #[cfg(not(windows))]
        {
            let _ = (width, height);
            None
        }
    }

    /// Whether the pointer moved, changed or appeared since the last frame.
    pub fn moved(&self, width: u32, height: u32) -> bool {
        self.pointer(width, height) != self.drawn
    }

    /// Blends the pointer into a frame showing the source.
    pub fn draw(&mut self, frame: &mut I420Buffer) {
        self.drawn = self.pointer(frame.width(), frame.height());
        let Some((x, y, handle)) = self.drawn else { return };
        if self.cached.as_ref().is_none_or(|(h, _)| *h != handle) {
            #[cfg(windows)]
            {
                self.cached = imp::render(handle).map(|image| (handle, image));
            }
        }
        if let Some((_, image)) = &self.cached {
            blend_i420(frame, image, x - image.hotspot.0, y - image.hotspot.1);
        }
    }
}

/// Alpha-blends `image` with its top left corner at (x, y) into an I420
/// frame (BT.601 limited range, like libyuv's conversions).
pub(crate) fn blend_i420(frame: &mut I420Buffer, image: &CursorImage, x: i32, y: i32) {
    let (fw, fh) = (frame.width() as i32, frame.height() as i32);
    let (sy, su, sv) = frame.strides();
    let (sy, su, sv) = (sy as usize, su as usize, sv as usize);
    let (py, pu, pv) = frame.data_mut();
    for cy in 0..image.height as i32 {
        let ty = y + cy;
        if ty < 0 || ty >= fh {
            continue;
        }
        for cx in 0..image.width as i32 {
            let tx = x + cx;
            if tx < 0 || tx >= fw {
                continue;
            }
            let [r, g, b, a] = image.rgba[(cy as u32 * image.width + cx as u32) as usize];
            if a == 0 {
                continue;
            }
            let (r, g, b, a) = (r as f32, g as f32, b as f32, a as f32 / 255.0);
            let mix = |old: u8, new: f32| (old as f32 * (1.0 - a) + new * a).round().clamp(0.0, 255.0) as u8;
            let yi = ty as usize * sy + tx as usize;
            py[yi] = mix(py[yi], 0.257 * r + 0.504 * g + 0.098 * b + 16.0);
            // One chroma sample per 2×2 block, taken from its top left pixel.
            if tx % 2 == 0 && ty % 2 == 0 {
                let ui = (ty / 2) as usize * su + (tx / 2) as usize;
                let vi = (ty / 2) as usize * sv + (tx / 2) as usize;
                pu[ui] = mix(pu[ui], -0.148 * r - 0.291 * g + 0.439 * b + 128.0);
                pv[vi] = mix(pv[vi], 0.439 * r - 0.368 * g - 0.071 * b + 128.0);
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetMonitorInfoW, GetObjectW, HBITMAP, HGDIOBJ, MONITORINFO, SelectObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, GetWindowRect, HCURSOR, HICON,
        ICONINFO,
    };

    use super::{CursorImage, ScreenSource, SourceKind};

    /// The screen rectangle the source shows (physical pixels).
    fn area(source: &ScreenSource) -> Option<RECT> {
        match source.kind {
            SourceKind::Window => {
                let mut rect = RECT::default();
                // SAFETY: any window handle; fails cleanly for a closed window.
                unsafe { GetWindowRect(HWND(source.id as *mut _), &mut rect) }.ok()?;
                Some(rect)
            }
            SourceKind::Screen => {
                let monitor = crate::hdr_capture::monitor_of(source)?;
                let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                // SAFETY: a sized struct for a monitor handle.
                unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool().then_some(info.rcMonitor)
            }
        }
    }

    /// Where the pointer is in a frame of `width × height` showing the
    /// source, and its cursor handle; `None` while it is hidden.
    pub fn pointer_in(source: &ScreenSource, width: u32, height: u32) -> Option<(i32, i32, isize)> {
        let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        // SAFETY: a sized struct.
        unsafe { GetCursorInfo(&mut info) }.ok()?;
        if info.flags.0 & CURSOR_SHOWING.0 == 0 || info.hCursor.is_invalid() {
            return None;
        }
        let rect = area(source)?;
        let (rw, rh) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
        let POINT { x, y } = info.ptScreenPos;
        let fx = (x - rect.left) as i64 * width as i64 / rw as i64;
        let fy = (y - rect.top) as i64 * height as i64 / rh as i64;
        Some((fx as i32, fy as i32, info.hCursor.0 as isize))
    }

    /// A 32-bit top-down bitmap filled with `fill` (BGRA) and its pixels.
    fn canvas(width: u32, height: u32, fill: u32) -> Option<(HBITMAP, *mut u32)> {
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: a valid header; the pixel memory belongs to the bitmap.
        let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        if bits.is_null() {
            return None;
        }
        let pixels = bits as *mut u32;
        // SAFETY: width × height pixels were just allocated.
        unsafe { std::slice::from_raw_parts_mut(pixels, (width * height) as usize).fill(fill) };
        Some((bitmap, pixels))
    }

    fn bitmap_size(bitmap: HBITMAP) -> Option<(u32, u32)> {
        let mut info = BITMAP::default();
        // SAFETY: fills a BITMAP for a bitmap handle.
        let n = unsafe {
            GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut c_void))
        };
        (n > 0).then_some((info.bmWidth as u32, info.bmHeight as u32))
    }

    /// The cursor drawn on `fill` (BGRA pixels).
    fn draw_on(cursor: HCURSOR, width: u32, height: u32, fill: u32) -> Option<Vec<u32>> {
        let (bitmap, pixels) = canvas(width, height, fill)?;
        // SAFETY: GDI objects made, used and released here.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bitmap.into());
            let drawn = DrawIconEx(dc, 0, 0, HICON(cursor.0), width as i32, height as i32, 0, None, DI_NORMAL);
            let out = drawn.is_ok().then(|| std::slice::from_raw_parts(pixels, (width * height) as usize).to_vec());
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            out
        }
    }

    /// Renders a cursor into an image with transparency.
    pub fn render(handle: isize) -> Option<CursorImage> {
        let cursor = HCURSOR(handle as *mut _);
        let mut icon = ICONINFO::default();
        // SAFETY: fills ICONINFO for a cursor handle; its bitmaps are ours to delete.
        unsafe { GetIconInfo(HICON(cursor.0), &mut icon) }.ok()?;
        let size = if !icon.hbmColor.is_invalid() {
            bitmap_size(icon.hbmColor)
        } else {
            // Monochrome: AND and XOR masks stacked in one bitmap.
            bitmap_size(icon.hbmMask).map(|(w, h)| (w, h / 2))
        };
        // SAFETY: the bitmaps GetIconInfo created.
        unsafe {
            if !icon.hbmColor.is_invalid() {
                let _ = DeleteObject(icon.hbmColor.into());
            }
            let _ = DeleteObject(icon.hbmMask.into());
        }
        let (width, height) = size.filter(|(w, h)| *w > 0 && *h > 0 && *w <= 256 && *h <= 256)?;
        let on_black = draw_on(cursor, width, height, 0xFF00_0000)?;
        let on_white = draw_on(cursor, width, height, 0xFFFF_FFFF)?;
        let rgba = on_black
            .iter()
            .zip(&on_white)
            .map(|(&b, &w)| {
                let channel = |p: u32, shift: u32| ((p >> shift) & 0xFF) as f32;
                // Over black a pixel shows color × alpha, over white that plus
                // (1 − alpha) × 255: the difference gives the alpha.
                let diff = (channel(w, 16) - channel(b, 16) + channel(w, 8) - channel(b, 8) + channel(w, 0)
                    - channel(b, 0))
                    / 3.0;
                let alpha = (255.0 - diff).clamp(0.0, 255.0);
                if alpha < 1.0 {
                    return [0, 0, 0, 0];
                }
                let un = |c: f32| (c * 255.0 / alpha).clamp(0.0, 255.0) as u8;
                [un(channel(b, 16)), un(channel(b, 8)), un(channel(b, 0)), alpha as u8]
            })
            .collect();
        Some(CursorImage { width, height, hotspot: (icon.xHotspot as i32, icon.yHotspot as i32), rgba })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_is_blended_where_it_belongs() {
        let mut frame = I420Buffer::new(8, 8);
        {
            let (y, u, v) = frame.data_mut();
            y.fill(16);
            u.fill(128);
            v.fill(128);
        }
        // A 2×2 white, opaque cursor at (4, 2), half off the bottom right never panics.
        let image = CursorImage { width: 2, height: 2, hotspot: (0, 0), rgba: vec![[255, 255, 255, 255]; 4] };
        blend_i420(&mut frame, &image, 4, 2);
        blend_i420(&mut frame, &image, 7, 7);
        blend_i420(&mut frame, &image, -1, -1);
        let (sy, _, _) = frame.strides();
        let (y, _, _) = frame.data();
        assert_eq!(y[2 * sy as usize + 4], 235, "white in limited range");
        assert_eq!(y[2 * sy as usize + 6], 16, "untouched next to it");
        assert_eq!(y[0], 235, "the part on screen of the one at (-1, -1)");
    }

    #[cfg(windows)]
    #[test]
    fn the_arrow_cursor_renders_with_transparency() {
        use windows::Win32::UI::WindowsAndMessaging::{IDC_ARROW, LoadCursorW};
        // SAFETY: loads a system cursor.
        let arrow = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap();
        let image = imp::render(arrow.0 as isize).expect("arrow renders");
        assert!(image.width >= 16 && image.height >= 16);
        assert!(image.rgba.iter().any(|p| p[3] == 255), "opaque parts");
        assert!(image.rgba.iter().any(|p| p[3] == 0), "transparent parts");
    }
}
