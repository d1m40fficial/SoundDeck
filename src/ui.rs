use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DT_CENTER,
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteObject, GetDC,
    GetTextExtentPoint32W, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, ReleaseDC,
};
use windows::core::w;

use crate::gfx::Canvas;
use crate::icons;
use crate::model::Track;
use crate::theme::{Theme, px, rgb};

pub const MAX_ROWS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hot {
    None,
    Body,
    Prev,
    Toggle,
    Next,
}

pub struct Fonts {
    pub strip_title: HFONT,
    pub strip_sub: HFONT,
    pub row_title: HFONT,
    pub row_sub: HFONT,

    pub glyph: HFONT,
    pub note: HFONT,
}

fn font(size: i32, weight: i32, face: windows::core::PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            -size,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            face,
        )
    }
}

impl Fonts {
    pub fn new(dpi: u32) -> Self {
        let ui = w!("Segoe UI Variable Text");
        Self {
            strip_title: font(px(12, dpi), 600, ui),
            strip_sub: font(px(10, dpi), 400, ui),
            row_title: font(px(12, dpi), 600, ui),
            row_sub: font(px(10, dpi), 400, ui),

            glyph: font(px(11, dpi), 400, w!("Segoe Fluent Icons")),
            note: font(px(13, dpi), 400, w!("Segoe UI Symbol")),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [
            self.strip_title,
            self.strip_sub,
            self.row_title,
            self.row_sub,
            self.glyph,
            self.note,
        ] {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
    }
}

pub fn measure(font: HFONT, text: &str) -> i32 {
    let buffer: Vec<u16> = text.encode_utf16().collect();
    if buffer.is_empty() {
        return 0;
    }
    unsafe {
        let dc: HDC = GetDC(None);
        if dc.is_invalid() {
            return 0;
        }
        let old = windows::Win32::Graphics::Gdi::SelectObject(dc, HGDIOBJ(font.0));
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &buffer, &mut size);
        windows::Win32::Graphics::Gdi::SelectObject(dc, old);
        let _ = ReleaseDC(None, dc);
        size.cx
    }
}

#[derive(Clone)]
pub struct StripLayout {
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub icon: RECT,
    pub title: RECT,
    pub subtitle: RECT,
    pub prev: RECT,
    pub toggle: RECT,
    pub next: RECT,
}

impl StripLayout {
    pub fn hit(&self, x: i32, y: i32) -> Hot {
        if inside(self.prev, x, y) {
            Hot::Prev
        } else if inside(self.toggle, x, y) {
            Hot::Toggle
        } else if inside(self.next, x, y) {
            Hot::Next
        } else if y >= 0 && y < self.height {
            Hot::Body
        } else {
            Hot::None
        }
    }
}

pub fn strip_layout(dpi: u32, fonts: &Fonts, track: &Track, bar_height: i32) -> StripLayout {
    let height = (bar_height - px(8, dpi)).clamp(px(26, dpi), px(46, dpi));
    let pad = px(7, dpi);
    let pad_right = px(4, dpi);
    let gap = px(6, dpi);
    let icon_size = px(20, dpi);
    let button = px(22, dpi);
    let button_gap = px(2, dpi);

    let title_text = if track.title.is_empty() {
        &track.app_name
    } else {
        &track.title
    };
    let sub_text = if track.artist.is_empty() {
        &track.app_name
    } else {
        &track.artist
    };
    let text_width = measure(fonts.strip_title, title_text)
        .max(measure(fonts.strip_sub, sub_text))
        .clamp(px(64, dpi), px(150, dpi));

    let width = pad + pad_right + icon_size + gap * 2 + text_width + button * 3 + button_gap * 2;
    let middle = height / 2;

    let icon = RECT {
        left: pad,
        top: middle - icon_size / 2,
        right: pad + icon_size,
        bottom: middle + icon_size / 2,
    };
    let text_left = icon.right + gap;
    let next = RECT {
        left: width - pad_right - button,
        top: middle - button / 2,
        right: width - pad_right,
        bottom: middle + button / 2,
    };
    let toggle = RECT {
        left: next.left - button_gap - button,
        top: next.top,
        right: next.left - button_gap,
        bottom: next.bottom,
    };
    let prev = RECT {
        left: toggle.left - button_gap - button,
        top: next.top,
        right: toggle.left - button_gap,
        bottom: next.bottom,
    };
    let text_right = prev.left - gap;
    let single_line = track.artist.is_empty();
    let title = if single_line {
        RECT {
            left: text_left,
            top: 0,
            right: text_right,
            bottom: height,
        }
    } else {
        RECT {
            left: text_left,
            top: px(3, dpi),
            right: text_right,
            bottom: middle + px(1, dpi),
        }
    };
    let subtitle = RECT {
        left: text_left,
        top: middle - px(1, dpi),
        right: text_right,
        bottom: height - px(2, dpi),
    };

    StripLayout {
        width,
        height,
        dpi,
        icon,
        title,
        subtitle,
        prev,
        toggle,
        next,
    }
}

pub struct PopupLayout {
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub rows: Vec<RECT>,
}

pub fn popup_layout(dpi: u32, rows: usize) -> PopupLayout {
    let rows = rows.min(MAX_ROWS);
    let width = px(336, dpi);
    let pad = px(8, dpi);
    let row = px(50, dpi);
    let height = pad * 2 + row * rows as i32;
    let rows = (0..rows)
        .map(|index| {
            let top = pad + row * index as i32;
            RECT {
                left: pad,
                top,
                right: width - pad,
                bottom: top + row,
            }
        })
        .collect();
    PopupLayout {
        width,
        height,
        dpi,
        rows,
    }
}

pub fn paint_strip(
    theme: &Theme,
    fonts: &Fonts,
    track: &Track,
    layout: &StripLayout,
    hot: Hot,
    marquee: i32,
    canvas: &Canvas,
) {
    canvas.fill(theme.chip);
    // No border: the strip should read as part of the taskbar itself.

    draw_identity(theme, fonts, track, layout.icon, canvas);

    let title_text = if track.title.is_empty() {
        &track.app_name
    } else {
        &track.title
    };
    let title_width = canvas.text_width(fonts.strip_title, title_text);
    let available = layout.title.right - layout.title.left;
    let offset = if title_width > available { marquee } else { 0 };
    canvas.text_scrolled(
        fonts.strip_title,
        title_text,
        layout.title,
        theme.text,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT,
        offset,
    );

    if !track.artist.is_empty() {
        canvas.text(
            fonts.strip_sub,
            &track.artist,
            layout.subtitle,
            theme.subtext,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }

    let pause = track.playback.is_playing();
    draw_media_button(
        theme,
        layout.dpi,
        layout.prev,
        MediaGlyph::Prev,
        track.can_prev,
        hot == Hot::Prev,
        canvas,
    );
    draw_media_button(
        theme,
        layout.dpi,
        layout.toggle,
        if pause {
            MediaGlyph::Pause
        } else {
            MediaGlyph::Play
        },
        track.can_toggle,
        hot == Hot::Toggle,
        canvas,
    );
    draw_media_button(
        theme,
        layout.dpi,
        layout.next,
        MediaGlyph::Next,
        track.can_next,
        hot == Hot::Next,
        canvas,
    );
}

#[derive(Clone, Copy)]
enum MediaGlyph {
    Prev,
    Play,
    Pause,
    Next,
}

fn draw_media_button(
    theme: &Theme,
    dpi: u32,
    rect: RECT,
    glyph: MediaGlyph,
    enabled: bool,
    hovered: bool,
    canvas: &Canvas,
) {
    if hovered && enabled {
        canvas.round_rect(rect, 6, Some(theme.button_hover), None);
    }
    let color = if enabled { theme.text } else { theme.subtext };
    draw_media_glyph(canvas, dpi, rect, glyph, color);
}

/// Vector media icons. Drawn with a ~2px pen at 100% so their weight matches
/// the taskbar's show-hidden-icons chevron instead of the thin font glyphs.
fn draw_media_glyph(canvas: &Canvas, dpi: u32, rect: RECT, glyph: MediaGlyph, color: COLORREF) {
    let scale = dpi as f32 / 96.0;
    let cx = (rect.left + rect.right) as f32 / 2.0;
    let cy = (rect.top + rect.bottom) as f32 / 2.0;
    let point = |x: f32, y: f32| POINT {
        x: (cx + x * scale).round() as i32,
        y: (cy + y * scale).round() as i32,
    };
    let stroke = (2.0 * scale).round().max(1.0) as i32;
    let bar = |left: f32, right: f32, half: f32| RECT {
        left: (cx + left * scale).round() as i32,
        top: (cy - half * scale).round() as i32,
        right: (cx + right * scale).round() as i32,
        bottom: (cy + half * scale).round() as i32,
    };

    match glyph {
        MediaGlyph::Play => {
            let points = [point(-3.0, -5.0), point(-3.0, 5.0), point(4.0, 0.0)];
            canvas.stroke_polygon(&points, color, stroke);
        }
        MediaGlyph::Pause => {
            canvas.round_rect(
                bar(-3.2, -1.2, 5.0),
                (1.0 * scale) as i32,
                Some(color),
                None,
            );
            canvas.round_rect(bar(1.2, 3.2, 5.0), (1.0 * scale) as i32, Some(color), None);
        }
        MediaGlyph::Prev => {
            // The taskbar's show-hidden-icons chevron, rotated left.
            let points = [point(2.0, -4.5), point(-2.0, 0.0), point(2.0, 4.5)];
            canvas.stroke_polyline(&points, color, stroke);
        }
        MediaGlyph::Next => {
            let points = [point(-2.0, -4.5), point(2.0, 0.0), point(-2.0, 4.5)];
            canvas.stroke_polyline(&points, color, stroke);
        }
    }
}

pub fn paint_popup(
    theme: &Theme,
    fonts: &Fonts,
    tracks: &[Track],
    selected: Option<usize>,
    hover: Option<usize>,
    layout: &PopupLayout,
    canvas: &Canvas,
) {
    canvas.fill(theme.chip);
    canvas.round_rect(
        RECT {
            left: 0,
            top: 0,
            right: layout.width,
            bottom: layout.height,
        },
        10,
        None,
        Some(theme.border),
    );

    for (index, rect) in layout.rows.iter().enumerate() {
        let Some(track) = tracks.get(index) else {
            break;
        };
        let is_selected = selected == Some(index);
        if is_selected {
            canvas.round_rect(*rect, 7, Some(theme.selected), None);
        } else if hover == Some(index) {
            canvas.round_rect(*rect, 7, Some(theme.hover), None);
        }
        if is_selected {
            let bar = RECT {
                left: rect.left + px(3, layout.dpi),
                top: rect.top + px(12, layout.dpi),
                right: rect.left + px(6, layout.dpi),
                bottom: rect.bottom - px(12, layout.dpi),
            };
            canvas.round_rect(bar, 2, Some(theme.accent), None);
        }

        let row_pad = px(12, layout.dpi);
        let icon_size = px(28, layout.dpi);
        let icon = RECT {
            left: rect.left + row_pad,
            top: rect.top + (rect.bottom - rect.top - icon_size) / 2,
            right: rect.left + row_pad + icon_size,
            bottom: rect.top + (rect.bottom - rect.top + icon_size) / 2,
        };
        draw_identity(theme, fonts, track, icon, canvas);

        let text_left = icon.right + px(10, layout.dpi);
        let text_right = rect.right - px(30, layout.dpi);
        let title_rect = RECT {
            left: text_left,
            top: rect.top + px(9, layout.dpi),
            right: text_right,
            bottom: rect.top + px(27, layout.dpi),
        };
        let title = if track.title.is_empty() {
            &track.app_name
        } else {
            &track.title
        };
        canvas.text(
            fonts.row_title,
            title,
            title_rect,
            theme.text,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );

        let sub_text = format!("{} · {}", track.app_name, track.playback.label());
        let sub_rect = RECT {
            left: text_left,
            top: rect.top + px(27, layout.dpi),
            right: text_right,
            bottom: rect.bottom - px(7, layout.dpi),
        };
        canvas.text(
            fonts.row_sub,
            &sub_text,
            sub_rect,
            theme.subtext,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );

        if is_selected {
            let glyph = RECT {
                left: rect.right - px(26, layout.dpi),
                top: rect.top + (rect.bottom - rect.top - px(16, layout.dpi)) / 2,
                right: rect.right - px(10, layout.dpi),
                bottom: rect.top + (rect.bottom - rect.top + px(16, layout.dpi)) / 2,
            };
            canvas.text(
                fonts.glyph,
                "\u{E73E}",
                glyph,
                theme.accent,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
        } else if track.playback.is_playing() {
            let dot = px(6, layout.dpi);
            let center_y = rect.top + (rect.bottom - rect.top) / 2;
            let center_x = rect.right - px(18, layout.dpi);
            let dot_rect = RECT {
                left: center_x - dot / 2,
                top: center_y - dot / 2,
                right: center_x + dot / 2,
                bottom: center_y + dot / 2,
            };
            canvas.round_rect(dot_rect, dot / 2, Some(theme.accent), None);
        }
    }
}

fn draw_identity(theme: &Theme, fonts: &Fonts, track: &Track, rect: RECT, canvas: &Canvas) {
    if let Some(art) = &track.art {
        canvas.image(&art.bgra, art.width, art.height, rect);
        return;
    }
    if let Some(icon) = icons::app_icon(&track.app_id) {
        canvas.icon(icon, rect);
        return;
    }
    canvas.round_rect(rect, 4, Some(theme.accent), None);
    canvas.text(
        fonts.note,
        "\u{266B}",
        rect,
        rgb(255, 255, 255),
        DT_SINGLELINE | DT_VCENTER | DT_CENTER,
    );
}

fn inside(rect: RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}
