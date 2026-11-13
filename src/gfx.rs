//! Tiny GDI+ wrapper: anti-aliased shapes and text, plus per-pixel-alpha
//! layered windows (soft shadows, real pill shapes) and smooth icons.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, UpdateLayeredWindow, HICON, ICONINFO, ULW_ALPHA};

const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;

pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// Multiply a colour's alpha by `k` (0..1).
pub fn fade(c: u32, k: f32) -> u32 {
    let a = ((c >> 24) as f32 * k.clamp(0.0, 1.0)).round() as u32;
    a << 24 | (c & 0x00FF_FFFF)
}

pub fn startup() {
    let mut token = 0usize;
    let input = GdiplusStartupInput { GdiplusVersion: 1, DebugEventCallback: 0, SuppressBackgroundThread: 0, SuppressExternalCodecs: 0 };
    unsafe {
        GdiplusStartup(&mut token, &input, null_mut());
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Light,
    Regular,
    Semibold,
    Bold,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

thread_local! {
    static FONTS: RefCell<HashMap<(u32, Weight), usize>> = RefCell::new(HashMap::new());
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn font(size: f32, weight: Weight) -> *mut GpFont {
    let key = ((size * 4.0).round() as u32, weight);
    FONTS.with(|f| {
        if let Some(&p) = f.borrow().get(&key) {
            return p as *mut GpFont;
        }
        let (fam, style) = match weight {
            Weight::Light => ("Segoe UI Light", FontStyleRegular),
            Weight::Regular => ("Segoe UI", FontStyleRegular),
            Weight::Semibold => ("Segoe UI Semibold", FontStyleRegular),
            Weight::Bold => ("Segoe UI", FontStyleBold),
        };
        let mut family: *mut GpFontFamily = null_mut();
        if GdipCreateFontFamilyFromName(w(fam).as_ptr(), null_mut(), &mut family) != Ok {
            GdipCreateFontFamilyFromName(w("Segoe UI").as_ptr(), null_mut(), &mut family);
        }
        let mut ft: *mut GpFont = null_mut();
        GdipCreateFont(family, key.0 as f32 / 4.0, style, UnitPixel, &mut ft);
        GdipDeleteFontFamily(family);
        f.borrow_mut().insert(key, ft as usize);
        ft
    })
}

pub struct Canvas {
    g: *mut GpGraphics,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.g);
        }
    }
}

impl Canvas {
    unsafe fn wrap(g: *mut GpGraphics) -> Canvas {
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHighQuality);
        GdipSetTextRenderingHint(g, TextRenderingHintAntiAliasGridFit);
        Canvas { g }
    }

    pub unsafe fn from_hdc(dc: HDC) -> Canvas {
        let mut g = null_mut();
        GdipCreateFromHDC(dc, &mut g);
        Canvas::wrap(g)
    }

    pub fn clear(&self, c: u32) {
        unsafe {
            GdipGraphicsClear(self.g, c);
        }
    }

    unsafe fn brush(c: u32) -> *mut GpSolidFill {
        let mut b = null_mut();
        GdipCreateSolidFill(c, &mut b);
        b
    }

    unsafe fn pen(c: u32, width: f32) -> *mut GpPen {
        let mut p = null_mut();
        GdipCreatePen1(c, width, UnitPixel, &mut p);
        GdipSetPenStartCap(p, LineCapRound);
        GdipSetPenEndCap(p, LineCapRound);
        GdipSetPenLineJoin(p, 2); // round
        p
    }

    unsafe fn rr_path(x: f32, y: f32, wd: f32, ht: f32, r: f32) -> *mut GpPath {
        let mut p = null_mut();
        GdipCreatePath(FillModeAlternate, &mut p);
        let d = (r.min(wd / 2.0).min(ht / 2.0) * 2.0).max(0.5);
        GdipAddPathArc(p, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(p, x + wd - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(p, x + wd - d, y + ht - d, d, d, 0.0, 90.0);
        GdipAddPathArc(p, x, y + ht - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(p);
        p
    }

    #[allow(dead_code)]
    pub fn fill_rect(&self, x: f32, y: f32, wd: f32, ht: f32, c: u32) {
        unsafe {
            let b = Self::brush(c);
            GdipFillRectangle(self.g, b as _, x, y, wd, ht);
            GdipDeleteBrush(b as _);
        }
    }

    pub fn fill_round(&self, x: f32, y: f32, wd: f32, ht: f32, r: f32, c: u32) {
        unsafe {
            let p = Self::rr_path(x, y, wd, ht, r);
            let b = Self::brush(c);
            GdipFillPath(self.g, b as _, p);
            GdipDeleteBrush(b as _);
            GdipDeletePath(p);
        }
    }

    pub fn stroke_round(&self, x: f32, y: f32, wd: f32, ht: f32, r: f32, width: f32, c: u32) {
        unsafe {
            let p = Self::rr_path(x, y, wd, ht, r);
            let pen = Self::pen(c, width);
            GdipDrawPath(self.g, pen, p);
            GdipDeletePen(pen);
            GdipDeletePath(p);
        }
    }

    /// Soft drop shadow under a rounded rect.
    pub fn shadow(&self, x: f32, y: f32, wd: f32, ht: f32, r: f32, spread: f32, strength: f32) {
        let steps = 6;
        for i in 0..steps {
            let e = spread * (steps - i) as f32 / steps as f32;
            let a = (strength * 255.0 / steps as f32 * 0.55) as u8;
            self.fill_round(x - e, y - e + spread * 0.35, wd + 2.0 * e, ht + 2.0 * e, r + e, argb(a, 0, 0, 0));
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, c: u32) {
        unsafe {
            let b = Self::brush(c);
            GdipFillEllipse(self.g, b as _, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeleteBrush(b as _);
        }
    }

    pub fn ring(&self, cx: f32, cy: f32, r: f32, width: f32, c: u32) {
        unsafe {
            let p = Self::pen(c, width);
            GdipDrawEllipse(self.g, p, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeletePen(p);
        }
    }

    /// Arc with round caps; angles in degrees, 0 = 3 o'clock, clockwise.
    pub fn arc(&self, cx: f32, cy: f32, r: f32, width: f32, start: f32, sweep: f32, c: u32) {
        if sweep.abs() < 0.5 {
            return;
        }
        unsafe {
            let p = Self::pen(c, width);
            GdipDrawArc(self.g, p, cx - r, cy - r, r * 2.0, r * 2.0, start, sweep.clamp(-360.0, 360.0));
            GdipDeletePen(p);
        }
    }

    /// Almond eye outline + pupil, `wd` wide.
    pub fn eye(&self, cx: f32, cy: f32, wd: f32, stroke: f32, c: u32, filled: Option<u32>) {
        unsafe {
            let hw = wd / 2.0;
            let k = wd * 0.34 / 0.75;
            let mut p = null_mut();
            GdipCreatePath(FillModeAlternate, &mut p);
            GdipAddPathBezier(p, cx - hw, cy, cx - hw * 0.45, cy - k, cx + hw * 0.45, cy - k, cx + hw, cy);
            GdipAddPathBezier(p, cx + hw, cy, cx + hw * 0.45, cy + k, cx - hw * 0.45, cy + k, cx - hw, cy);
            GdipClosePathFigure(p);
            if let Some(fc) = filled {
                let b = Self::brush(fc);
                GdipFillPath(self.g, b as _, p);
                GdipDeleteBrush(b as _);
            }
            if stroke > 0.0 {
                let pen = Self::pen(c, stroke);
                GdipDrawPath(self.g, pen, p);
                GdipDeletePen(pen);
            }
            GdipDeletePath(p);
        }
        self.fill_circle(cx, cy, wd * 0.19, c);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text(&self, s: &str, x: f32, y: f32, wd: f32, ht: f32, size: f32, weight: Weight, c: u32, align: Align, wrap: bool) {
        if s.is_empty() || wd <= 0.0 || ht <= 0.0 {
            return;
        }
        unsafe {
            let f = font(size, weight);
            let mut fmt = null_mut();
            GdipCreateStringFormat(0, 0, &mut fmt);
            GdipSetStringFormatAlign(
                fmt,
                match align {
                    Align::Left => StringAlignmentNear,
                    Align::Center => StringAlignmentCenter,
                    Align::Right => StringAlignmentFar,
                },
            );
            GdipSetStringFormatLineAlign(fmt, StringAlignmentCenter);
            GdipSetStringFormatTrimming(fmt, StringTrimmingEllipsisCharacter);
            if !wrap {
                GdipSetStringFormatFlags(fmt, StringFormatFlagsNoWrap);
            }
            let b = Self::brush(c);
            let ws: Vec<u16> = s.encode_utf16().collect();
            let r = RectF { X: x, Y: y, Width: wd, Height: ht };
            GdipDrawString(self.g, ws.as_ptr(), ws.len() as i32, f, &r, fmt, b as _);
            GdipDeleteBrush(b as _);
            GdipDeleteStringFormat(fmt);
        }
    }
}

/// Render into a 32-bit DIB with GDI+ and push it to a WS_EX_LAYERED window
/// (per-pixel alpha: true soft edges and shadows).
pub unsafe fn present_layered(h: HWND, x: i32, y: i32, wd: i32, ht: i32, draw: impl FnOnce(&Canvas)) {
    let screen = GetDC(null_mut());
    let mem = CreateCompatibleDC(screen);
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = wd;
    bi.bmiHeader.biHeight = -ht;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = null_mut();
    let dib = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if !dib.is_null() && !bits.is_null() {
        let old = SelectObject(mem, dib as _);
        let mut bmp: *mut GpBitmap = null_mut();
        GdipCreateBitmapFromScan0(wd, ht, wd * 4, PIXEL_FORMAT_32BPP_PARGB, bits as *const u8, &mut bmp);
        if !bmp.is_null() {
            let mut g = null_mut();
            GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
            {
                let c = Canvas::wrap(g);
                c.clear(0);
                draw(&c);
            }
            GdipDisposeImage(bmp as *mut GpImage);
            let pos = POINT { x, y };
            let size = SIZE { cx: wd, cy: ht };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(h, screen, &pos, &size, mem, &src, 0, &blend, ULW_ALPHA);
        }
        SelectObject(mem, old);
        DeleteObject(dib as _);
    }
    DeleteDC(mem);
    ReleaseDC(null_mut(), screen);
}

/// Smooth 32-bit app icon: teal disc with a white eye.
pub unsafe fn make_icon(size: i32, accent: u32) -> HICON {
    let screen = GetDC(null_mut());
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = size;
    bi.bmiHeader.biHeight = -size;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    let mut bits: *mut c_void = null_mut();
    let color = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    ReleaseDC(null_mut(), screen);
    if color.is_null() {
        return null_mut();
    }
    let mut bmp: *mut GpBitmap = null_mut();
    GdipCreateBitmapFromScan0(size, size, size * 4, PIXEL_FORMAT_32BPP_ARGB, bits as *const u8, &mut bmp);
    if !bmp.is_null() {
        let mut g = null_mut();
        GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
        {
            let c = Canvas::wrap(g);
            c.clear(0);
            draw_icon(&c, size as f32, accent);
        }
        GdipDisposeImage(bmp as *mut GpImage);
    }
    let mask_bits = vec![0u8; ((size as usize + 15) / 16 * 2) * size as usize];
    let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr() as _);
    let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&ii);
    DeleteObject(color as _);
    DeleteObject(mask as _);
    icon
}

/// Draw into an offscreen ARGB bitmap and save it as PNG (used by --preview).
pub unsafe fn render_png(path: &std::path::Path, wd: i32, ht: i32, draw: impl FnOnce(&Canvas)) -> bool {
    let mut bmp: *mut GpBitmap = null_mut();
    GdipCreateBitmapFromScan0(wd, ht, 0, PIXEL_FORMAT_32BPP_PARGB, std::ptr::null(), &mut bmp);
    if bmp.is_null() {
        return false;
    }
    let mut g = null_mut();
    GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g);
    {
        let c = Canvas::wrap(g);
        c.clear(0);
        draw(&c);
    }
    // PNG encoder {557CF406-1A04-11D3-9A73-0000F81EF32E}
    let png = windows_sys::core::GUID { data1: 0x557C_F406, data2: 0x1A04, data3: 0x11D3, data4: [0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E] };
    let ok = GdipSaveImageToFile(bmp as *mut GpImage, w(&path.to_string_lossy()).as_ptr(), &png, std::ptr::null()) == Ok;
    GdipDisposeImage(bmp as *mut GpImage);
    ok
}

/// The app mark: teal disc with a white eye.
pub fn draw_icon(c: &Canvas, s: f32, accent: u32) {
    c.fill_circle(s / 2.0, s / 2.0, s / 2.0 - 0.5, accent);
    c.eye(s / 2.0, s / 2.0, s * 0.70, 0.0, argb(255, 14, 40, 38), Some(argb(255, 255, 255, 255)));
}
