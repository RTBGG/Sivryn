//! Small icons drawn at run time for the notification area and the taskbar:
//! the Sivryn mark with a status dot or a voice bubble, and symbols from
//! Windows' own icon font (as Discord's taskbar buttons use).

/// An image with straight (not premultiplied) alpha.
#[derive(Clone)]
pub struct Image {
    pub size: u32,
    pub rgba: Vec<[u8; 4]>,
}

impl Image {
    fn transparent(size: u32) -> Self {
        Self { size, rgba: vec![[0; 4]; (size * size) as usize] }
    }
}

/// The Sivryn mark, scaled down to `size` × `size`.
pub fn logo(size: u32) -> Image {
    const SRC: u32 = 64;
    let src = include_bytes!("../assets/icons/icon-64.rgba");
    let mut out = Image::transparent(size);
    let scale = SRC as f32 / size as f32;
    for y in 0..size {
        for x in 0..size {
            // Average the source pixels the target pixel covers (alpha-weighted).
            let (x0, y0) = ((x as f32 * scale) as u32, (y as f32 * scale) as u32);
            let (x1, y1) =
                ((((x + 1) as f32 * scale).ceil() as u32).min(SRC), (((y + 1) as f32 * scale).ceil() as u32).min(SRC));
            let mut sum = [0f32; 4];
            let mut n = 0f32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * SRC + sx) * 4) as usize;
                    let a = src[i + 3] as f32;
                    sum[0] += src[i] as f32 * a;
                    sum[1] += src[i + 1] as f32 * a;
                    sum[2] += src[i + 2] as f32 * a;
                    sum[3] += a;
                    n += 1.0;
                }
            }
            if sum[3] > 0.0 {
                out.rgba[(y * size + x) as usize] =
                    [(sum[0] / sum[3]) as u8, (sum[1] / sum[3]) as u8, (sum[2] / sum[3]) as u8, (sum[3] / n) as u8];
            }
        }
    }
    out
}

/// How much of the pixel at (x, y) a disc covers (anti-aliased).
fn disc(x: u32, y: u32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
    (r + 0.5 - d).clamp(0.0, 1.0)
}

fn blend(px: &mut [u8; 4], color: [u8; 3], coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let a = px[3] as f32 / 255.0;
    let out_a = coverage + a * (1.0 - coverage);
    for c in 0..3 {
        px[c] = ((color[c] as f32 * coverage + px[c] as f32 * a * (1.0 - coverage)) / out_a.max(1e-6)) as u8;
    }
    px[3] = (out_a * 255.0) as u8;
}

/// A badge in the bottom right corner: cut out of the image with a gap,
/// then a dot, or a ring when `filled` is false.
pub fn badge(image: &mut Image, color: [u8; 3], radius: f32, filled: bool) {
    let n = image.size as f32;
    let gap = (n * 0.09).max(1.0);
    let (cx, cy) = (n - radius - 0.5, n - radius - 0.5);
    for y in 0..image.size {
        for x in 0..image.size {
            let px = &mut image.rgba[(y * image.size + x) as usize];
            let cut = disc(x, y, cx, cy, radius + gap);
            px[3] = (px[3] as f32 * (1.0 - cut)) as u8;
            let mut coverage = disc(x, y, cx, cy, radius);
            if !filled {
                let stroke = (radius * 0.38).max(1.2);
                coverage -= disc(x, y, cx, cy, radius - stroke);
            }
            blend(px, color, coverage.max(0.0));
        }
    }
}

/// A strike-through from top left to bottom right, with a gap beside it
/// (an "off" state, like a muted camera).
pub fn strike(image: &mut Image, color: [u8; 3]) {
    let n = image.size as f32;
    let (x0, y0, x1, y1) = (n * 0.14, n * 0.14, n * 0.86, n * 0.86);
    let width = (n * 0.085).max(1.2);
    let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    for y in 0..image.size {
        for x in 0..image.size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance to the segment; the gap is a wider band on one side.
            let t = (((px - x0) * (x1 - x0) + (py - y0) * (y1 - y0)) / (len * len)).clamp(0.0, 1.0);
            let (qx, qy) = (x0 + t * (x1 - x0), y0 + t * (y1 - y0));
            let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
            let side = (px - x0) * (y1 - y0) - (py - y0) * (x1 - x0);
            let pixel = &mut image.rgba[(y * image.size + x) as usize];
            if side > 0.0 {
                let gap = (width * 1.6 + 0.5 - d).clamp(0.0, 1.0);
                pixel[3] = (pixel[3] as f32 * (1.0 - gap)) as u8;
            }
            blend(pixel, color, (width / 2.0 + 0.5 - d).clamp(0.0, 1.0));
        }
    }
}

#[cfg(windows)]
pub use imp::{glyph, to_hicon};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{COLORREF, RECT};
    use windows::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FW_NORMAL, HBITMAP,
        OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};
    use windows::core::PCWSTR;

    use super::Image;

    /// A 32-bit top-down bitmap and a pointer to its pixels.
    fn dib(size: u32) -> Option<(HBITMAP, *mut u32)> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: a valid header; `bits` receives the pixel memory.
        let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        (!bits.is_null()).then_some((bitmap, bits as *mut u32))
    }

    /// The icon font: Segoe Fluent Icons on Windows 11, MDL2 before (same
    /// code points for these symbols).
    fn icon_font() -> &'static str {
        let windir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_default();
        if windir.join("Fonts").join("SegoeIcons.ttf").exists() { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }
    }

    /// A symbol from Windows' icon font, in `color`.
    pub fn glyph(symbol: char, size: u32, color: [u8; 3]) -> Image {
        let mut out = Image { size, rgba: vec![[0; 4]; (size * size) as usize] };
        let Some((bitmap, bits)) = dib(size) else { return out };
        let face: Vec<u16> = icon_font().encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: GDI objects are created, used on this thread and released.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let old_bitmap = SelectObject(dc, bitmap.into());
            let font = CreateFontW(
                -((size as f32 * 0.82) as i32),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32,
                PCWSTR(face.as_ptr()),
            );
            let old_font = SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut text: Vec<u16> = symbol.to_string().encode_utf16().collect();
            let mut rect = RECT { left: 0, top: 0, right: size as i32, bottom: size as i32 };
            DrawTextW(dc, &mut text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            // White on black: the brightness is the coverage.
            let pixels = std::slice::from_raw_parts(bits, (size * size) as usize);
            for (out, px) in out.rgba.iter_mut().zip(pixels) {
                let coverage = (px & 0xFF).max((px >> 8) & 0xFF).max((px >> 16) & 0xFF) as u8;
                *out = [color[0], color[1], color[2], coverage];
            }
            SelectObject(dc, old_font);
            SelectObject(dc, old_bitmap);
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
        }
        out
    }

    /// A Windows icon from the image (the caller destroys it).
    pub fn to_hicon(image: &Image) -> Option<HICON> {
        let size = image.size;
        let (color, bits) = dib(size)?;
        // SAFETY: `bits` points at size × size pixels of the bitmap just made.
        unsafe {
            let pixels = std::slice::from_raw_parts_mut(bits, (size * size) as usize);
            for (out, [r, g, b, a]) in pixels.iter_mut().zip(&image.rgba) {
                *out = (*a as u32) << 24 | (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
            }
            let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
            let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
            let icon = CreateIconIndirect(&info).ok();
            let _ = DeleteObject(mask.into());
            let _ = DeleteObject(color.into());
            icon
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_cut_out_and_draw_in_the_corner() {
        let mut image = logo(32);
        assert!(image.rgba.iter().any(|p| p[3] > 200), "the mark is opaque somewhere");
        badge(&mut image, [35, 165, 90], 6.0, true);
        let corner = image.rgba[(25 * 32 + 25) as usize];
        assert_eq!(&corner[..3], &[35, 165, 90]);
        assert!(corner[3] > 240);
        // A ring leaves its middle empty.
        let mut ring = Image::transparent(32);
        badge(&mut ring, [35, 165, 90], 8.0, false);
        let middle = ring.rgba[(24 * 32 + 24) as usize];
        assert!(middle[3] < 40, "{middle:?}");
    }
}
