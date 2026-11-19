use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt,
    CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_NOPREFIX, DeleteDC, DeleteObject,
    DrawTextW, FillRect, GetDC, GetStockObject, GetTextExtentPoint32W, HALFTONE, HBITMAP, HDC,
    HFONT, HGDIOBJ, IntersectClipRect, NULL_BRUSH, NULL_PEN, PS_ENDCAP_ROUND, PS_GEOMETRIC,
    PS_JOIN_ROUND, PS_SOLID, Polygon, Polyline, ReleaseDC, RestoreDC, RoundRect, SRCCOPY, SaveDC,
    SelectClipRgn, SelectObject, SetBkMode, SetStretchBltMode, SetTextColor, StretchBlt,
    StretchDIBits, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DrawIconEx, HICON, ULW_ALPHA, UpdateLayeredWindow,
};

/// Double-buffered 32-bit surface used for every widget window.
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let mut info = BITMAPINFO::default();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height; // top-down
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB.0;
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            Some(Self {
                width,
                height,
                dc,
                bitmap,
                previous,
                bits: bits as *mut u8,
            })
        }
    }

    /// Publishes the surface through `UpdateLayeredWindow`.
    ///
    /// The Windows 11 taskbar is composed via DirectComposition, which draws over
    /// ordinary GDI children. A layered child with per-pixel alpha is the only
    /// plain-Win32 way to sit visibly (and receive input) on top of it.
    pub fn present(&self, hwnd: HWND, radius: i32) {
        if self.bits.is_null() {
            return;
        }
        unsafe {
            // GDI leaves the alpha channel alone, so rebuild it: the chip is an
            // opaque rounded rectangle, which keeps text antialiasing crisp.
            let stride = self.width as usize * 4;
            for y in 0..self.height as usize {
                for x in 0..self.width as usize {
                    *self.bits.add(y * stride + x * 4 + 3) = 255;
                }
            }

            let r = radius.min(self.width / 2).min(self.height / 2);
            if r > 0 {
                let w = self.width;
                let h = self.height;
                let corners = [
                    (0, 0, r, r),
                    (w - r, 0, w - r, r),
                    (0, h - r, r, h - r),
                    (w - r, h - r, w - r, h - r),
                ];
                for (box_x, box_y, center_x, center_y) in corners {
                    for y in box_y..box_y + r {
                        for x in box_x..box_x + r {
                            let dx = x as f32 + 0.5 - center_x as f32;
                            let dy = y as f32 + 0.5 - center_y as f32;
                            let distance = (dx * dx + dy * dy).sqrt();
                            let coverage = (r as f32 - distance + 0.5).clamp(0.0, 1.0);
                            let index = y as usize * stride + x as usize * 4 + 3;
                            *self.bits.add(index) = (coverage * 255.0) as u8;
                        }
                    }
                }
            }

            let screen = GetDC(None);
            let size = SIZE {
                cx: self.width,
                cy: self.height,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                None,
                Some(&size),
                Some(self.dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = ReleaseDC(None, screen);
        }
    }

    pub fn fill(&self, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let rect = RECT {
                left: 0,
                top: 0,
                right: self.width,
                bottom: self.height,
            };
            FillRect(self.dc, &rect, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    /// Mirrors a horizontal band of the canvas directly below itself.
    ///
    /// Chevrons are drawn once in the upper half and flipped down pixel-for-pixel,
    /// so GDI's line rasterizer cannot make the two halves differ.
    pub fn mirror_below(&self, source: RECT, dest_top: i32) {
        let width = source.right - source.left;
        let height = source.bottom - source.top;
        if width <= 0 || height <= 0 {
            return;
        }
        unsafe {
            let mem = CreateCompatibleDC(Some(self.dc));
            let bitmap = CreateCompatibleBitmap(self.dc, width, height);
            if mem.is_invalid() || bitmap.is_invalid() {
                if !bitmap.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(bitmap.0));
                }
                if !mem.is_invalid() {
                    let _ = DeleteDC(mem);
                }
                return;
            }
            let old = SelectObject(mem, HGDIOBJ(bitmap.0));
            let _ = BitBlt(
                mem,
                0,
                0,
                width,
                height,
                Some(self.dc),
                source.left,
                source.top,
                SRCCOPY,
            );
            let _ = StretchBlt(
                self.dc,
                source.left,
                dest_top,
                width,
                height,
                Some(mem),
                0,
                height,
                width,
                -height,
                SRCCOPY,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
        }
    }

    /// Fills a closed polygon without an outline (the play triangle).
    pub fn fill_polygon(&self, points: &[POINT], color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let old_brush = SelectObject(self.dc, HGDIOBJ(brush.0));
            let old_pen = SelectObject(self.dc, GetStockObject(NULL_PEN));
            let _ = Polygon(self.dc, points);
            SelectObject(self.dc, old_pen);
            SelectObject(self.dc, old_brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }
    }

    /// Draws an open polyline (used for the chevron-style prev/next buttons).
    pub fn stroke_polyline(&self, points: &[POINT], color: COLORREF, width: i32) {
        unsafe {
            let pen = CreatePen(
                PS_GEOMETRIC | PS_SOLID | PS_JOIN_ROUND | PS_ENDCAP_ROUND,
                width.max(1),
                color,
            );
            let old_pen = SelectObject(self.dc, HGDIOBJ(pen.0));
            let old_brush = SelectObject(self.dc, GetStockObject(NULL_BRUSH));
            let _ = Polyline(self.dc, points);
            SelectObject(self.dc, old_brush);
            SelectObject(self.dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }

    /// Draws a closed outline with a thick pen, used for the media buttons so
    /// their weight matches the taskbar's own chevron.
    pub fn stroke_polygon(&self, points: &[POINT], color: COLORREF, width: i32) {
        unsafe {
            let pen = CreatePen(
                PS_GEOMETRIC | PS_SOLID | PS_JOIN_ROUND | PS_ENDCAP_ROUND,
                width.max(1),
                color,
            );
            let old_pen = SelectObject(self.dc, HGDIOBJ(pen.0));
            let old_brush = SelectObject(self.dc, GetStockObject(NULL_BRUSH));
            let _ = Polygon(self.dc, points);
            SelectObject(self.dc, old_brush);
            SelectObject(self.dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }

    /// Fills a rounded rectangle and/or strokes its border.
    pub fn round_rect(
        &self,
        rect: RECT,
        radius: i32,
        fill: Option<COLORREF>,
        border: Option<COLORREF>,
    ) {
        unsafe {
            // RoundRect always fills with the selected brush, so "no fill" must
            // explicitly select the stock NULL_BRUSH (otherwise GDI paints white).
            let created_brush = fill.map(|color| CreateSolidBrush(color));
            let created_pen = border.map(|color| CreatePen(PS_SOLID, 1, color));
            let brush = created_brush
                .map(|brush| HGDIOBJ(brush.0))
                .unwrap_or_else(|| GetStockObject(NULL_BRUSH));
            let pen = created_pen
                .map(|pen| HGDIOBJ(pen.0))
                .unwrap_or_else(|| GetStockObject(NULL_PEN));
            let old_brush = SelectObject(self.dc, brush);
            let old_pen = SelectObject(self.dc, pen);
            let _ = RoundRect(
                self.dc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius * 2,
                radius * 2,
            );
            SelectObject(self.dc, old_pen);
            SelectObject(self.dc, old_brush);
            if let Some(pen) = created_pen {
                let _ = DeleteObject(HGDIOBJ(pen.0));
            }
            if let Some(brush) = created_brush {
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
    }

    pub fn text(
        &self,
        font: HFONT,
        text: &str,
        mut rect: RECT,
        color: COLORREF,
        flags: DRAW_TEXT_FORMAT,
    ) {
        let mut buffer: Vec<u16> = text.encode_utf16().collect();
        if buffer.is_empty() {
            return;
        }
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, color);
            let _ = DrawTextW(self.dc, &mut buffer, &mut rect, flags | DT_NOPREFIX);
            SelectObject(self.dc, old);
        }
    }

    /// Draws text clipped to `rect`, shifted left by `offset` (marquee).
    pub fn text_scrolled(
        &self,
        font: HFONT,
        text: &str,
        mut rect: RECT,
        color: COLORREF,
        flags: DRAW_TEXT_FORMAT,
        offset: i32,
    ) {
        let mut buffer: Vec<u16> = text.encode_utf16().collect();
        if buffer.is_empty() {
            return;
        }
        unsafe {
            let saved = SaveDC(self.dc);
            let _ = IntersectClipRect(self.dc, rect.left, rect.top, rect.right, rect.bottom);
            rect.left -= offset;
            // DrawText clips to its rect too; widen it so shifting the origin does
            // not cut the string, while the DC clip keeps the visible window.
            rect.right = rect.left + self.text_width(font, text) + 4;
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, color);
            let _ = DrawTextW(self.dc, &mut buffer, &mut rect, flags | DT_NOPREFIX);
            SelectObject(self.dc, old);
            let _ = RestoreDC(self.dc, saved);
        }
    }

    pub fn text_width(&self, font: HFONT, text: &str) -> i32 {
        let buffer: Vec<u16> = text.encode_utf16().collect();
        if buffer.is_empty() {
            return 0;
        }
        unsafe {
            let old = SelectObject(self.dc, HGDIOBJ(font.0));
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(self.dc, &buffer, &mut size);
            SelectObject(self.dc, old);
            size.cx
        }
    }

    pub fn image(&self, bgra: &[u8], width: u32, height: u32, dst: RECT) {
        if width == 0 || height == 0 {
            return;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width as i32;
        info.bmiHeader.biHeight = -(height as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        unsafe {
            SetStretchBltMode(self.dc, HALFTONE);
            let _ = StretchDIBits(
                self.dc,
                dst.left,
                dst.top,
                dst.right - dst.left,
                dst.bottom - dst.top,
                0,
                0,
                width as i32,
                height as i32,
                Some(bgra.as_ptr() as *const core::ffi::c_void),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }

    pub fn icon(&self, icon: HICON, rect: RECT) {
        unsafe {
            let _ = DrawIconEx(
                self.dc,
                rect.left,
                rect.top,
                icon,
                rect.right - rect.left,
                rect.bottom - rect.top,
                0,
                None,
                DI_NORMAL,
            );
        }
    }

    fn with_round_clip<F: FnOnce()>(&self, rect: RECT, radius: i32, draw: F) {
        unsafe {
            let saved = SaveDC(self.dc);
            let region = CreateRoundRectRgn(
                rect.left,
                rect.top,
                rect.right + 1,
                rect.bottom + 1,
                radius * 2,
                radius * 2,
            );
            if !region.is_invalid() {
                SelectClipRgn(self.dc, Some(region));
            }
            draw();
            let _ = RestoreDC(self.dc, saved);
            if !region.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(region.0));
            }
        }
    }

    pub fn image_rounded(&self, bgra: &[u8], width: u32, height: u32, dst: RECT, radius: i32) {
        self.with_round_clip(dst, radius, || self.image(bgra, width, height, dst));
    }

    pub fn icon_rounded(&self, icon: HICON, rect: RECT, radius: i32) {
        self.with_round_clip(rect, radius, || self.icon(icon, rect));
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}
