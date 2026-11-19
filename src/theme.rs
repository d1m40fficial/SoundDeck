use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS};
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,

    pub chip: COLORREF,
    pub border: COLORREF,
    pub text: COLORREF,
    pub subtext: COLORREF,
    pub accent: COLORREF,
    pub hover: COLORREF,
    pub selected: COLORREF,
    pub button_hover: COLORREF,
}

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn mix(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let channel = |value: u32, shift: u32| ((value >> shift) & 0xFF) as f32;
    let blend = |sa: u32, sb: u32| {
        let v = channel(a.0, sa) * (1.0 - t) + channel(b.0, sb) * t;
        (v.round() as u32).min(255)
    };
    rgb(blend(0, 0) as u8, blend(8, 8) as u8, blend(16, 16) as u8)
}

fn registry_dword(subkey: &str, name: &str) -> Option<u32> {
    let subkey: Vec<u16> = subkey.encode_utf16().chain([0]).collect();
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

fn system_accent() -> Option<COLORREF> {
    let mut argb = 0u32;
    let mut opaque = windows::core::BOOL(0);
    unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.ok()?;
    // 0xAARRGGBB -> COLORREF 0x00BBGGRR
    let r = ((argb >> 16) & 0xFF) as u8;
    let g = ((argb >> 8) & 0xFF) as u8;
    let b = (argb & 0xFF) as u8;
    Some(rgb(r, g, b))
}

impl Theme {
    pub fn load() -> Self {
        let dark = registry_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "SystemUsesLightTheme",
        )
        .map(|v| v == 0)
        .unwrap_or(true);

        let accent = system_accent().unwrap_or(rgb(76, 194, 255));
        // Keep the accent readable on the current taskbar shade.
        let accent = if dark {
            mix(accent, rgb(255, 255, 255), 0.18)
        } else {
            mix(accent, rgb(0, 0, 0), 0.12)
        };

        let theme = if dark {
            Self {
                dark,
                chip: rgb(32, 32, 32),
                border: rgb(60, 60, 60),
                text: rgb(248, 248, 248),
                subtext: rgb(166, 166, 166),
                accent,
                hover: rgb(56, 56, 56),
                selected: rgb(56, 56, 56),
                button_hover: rgb(60, 60, 60),
            }
        } else {
            Self {
                dark,
                chip: rgb(243, 243, 243),
                border: rgb(214, 214, 214),
                text: rgb(23, 23, 23),
                subtext: rgb(97, 97, 97),
                accent,
                hover: rgb(234, 234, 234),
                selected: rgb(234, 234, 234),
                button_hover: rgb(232, 232, 232),
            }
        };
        let mut theme = theme;
        theme.set_chip(theme.chip);
        theme
    }

    /// Recolors the widget to blend with the actual taskbar shade.
    pub fn set_chip(&mut self, chip: COLORREF) {
        self.chip = chip;
        let r = (chip.0 & 0xFF) as f32;
        let g = ((chip.0 >> 8) & 0xFF) as f32;
        let b = ((chip.0 >> 16) & 0xFF) as f32;
        let luma = 0.299 * r + 0.587 * g + 0.114 * b;
        let light_chip = luma > 140.0;
        let tint = if light_chip {
            rgb(0, 0, 0)
        } else {
            rgb(255, 255, 255)
        };
        if light_chip {
            self.text = rgb(23, 23, 23);
            self.subtext = rgb(90, 90, 90);
        } else {
            self.text = rgb(248, 248, 248);
            self.subtext = rgb(166, 166, 166);
        }
        self.border = mix(chip, tint, 0.20);
        self.hover = mix(chip, tint, if light_chip { 0.06 } else { 0.08 });
        self.button_hover = mix(chip, tint, 0.14);
        self.selected = mix(chip, self.accent, if light_chip { 0.16 } else { 0.18 });
    }
}

pub fn px(value: i32, dpi: u32) -> i32 {
    ((value as f32) * dpi as f32 / 96.0).round() as i32
}
