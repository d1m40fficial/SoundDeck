use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    IShellItem, SHCreateItemFromParsingName, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
    SHGetFileInfoW, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::PCWSTR;

static CACHE: OnceLock<Mutex<HashMap<String, Option<isize>>>> = OnceLock::new();
static LABELS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Returns a 32x32 shell icon for the app that owns a media session.
///
/// `shell:AppsFolder\<AUMID>` resolves both packaged (UWP/MSIX) and most unpackaged
/// apps that publish media controls, so no vendor executable paths are hardcoded.
pub fn app_icon(app_id: &str) -> Option<HICON> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().ok()?;
    if let Some(hit) = cache.get(app_id) {
        return hit.map(|raw| HICON(raw as *mut core::ffi::c_void));
    }

    let icon = load_icon(app_id);
    cache.insert(app_id.to_string(), icon.map(|icon| icon.0 as isize));
    icon
}

fn load_icon(app_id: &str) -> Option<HICON> {
    let candidates = [format!("shell:AppsFolder\\{app_id}"), app_id.to_string()];
    for candidate in candidates {
        let wide: Vec<u16> = candidate.encode_utf16().chain([0]).collect();
        let mut info = SHFILEINFOW::default();
        let ok = unsafe {
            SHGetFileInfoW(
                PCWSTR(wide.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            )
        };
        if ok != 0 && !info.hIcon.is_invalid() {
            return Some(info.hIcon);
        }
    }
    None
}

/// Human-readable app name for the popup rows and the tray tooltip.
pub fn app_label(app_id: &str) -> String {
    let cache = LABELS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(label) = cache.get(app_id) {
            return label.clone();
        }
    }

    let label = resolve_label(app_id);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(app_id.to_string(), label.clone());
    }
    label
}

fn resolve_label(app_id: &str) -> String {
    let lower = app_id.to_lowercase();
    const KNOWN: &[(&str, &str)] = &[
        ("spotify", "Spotify"),
        ("msedge", "Microsoft Edge"),
        ("chrome", "Google Chrome"),
        ("firefox", "Firefox"),
        ("vivaldi", "Vivaldi"),
        ("opera", "Opera"),
        ("brave", "Brave"),
        ("yandex", "Yandex Music"),
        ("zunemusic", "Media Player"),
        ("media.player", "Media Player"),
        ("wmplayer", "Windows Media Player"),
        ("vlc", "VLC"),
        ("foobar2000", "foobar2000"),
        ("aimp", "AIMP"),
        ("musicbee", "MusicBee"),
        ("itunes", "iTunes"),
        ("applemusic", "Apple Music"),
        ("deezer", "Deezer"),
        ("tidal", "TIDAL"),
        ("telegram", "Telegram"),
        ("discord", "Discord"),
        ("potplayer", "PotPlayer"),
        ("mpc-hc", "MPC-HC"),
        ("amazonmusic", "Amazon Music"),
        ("soundcloud", "SoundCloud"),
    ];
    for (needle, label) in KNOWN {
        if lower.contains(needle) {
            return (*label).to_string();
        }
    }

    // Ask the shell for the registered display name (Firefox uses a per-install
    // 16-hex AUMID, for example), then fall back to the id itself.
    if let Some(label) = shell_label(app_id) {
        return label;
    }

    let tail = app_id.rsplit('!').next().unwrap_or(app_id);
    let tail = tail.rsplit(['\\', '/']).next().unwrap_or(tail);
    let tail = tail.strip_suffix(".exe").unwrap_or(tail);
    let tail = tail.split('_').next().unwrap_or(tail);
    if tail.is_empty() || (tail.len() >= 8 && tail.chars().all(|c| c.is_ascii_hexdigit())) {
        return "Media".to_string();
    }
    let mut chars = tail.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Media".to_string(),
    }
}

fn shell_label(app_id: &str) -> Option<String> {
    let path = format!("shell:AppsFolder\\{app_id}");
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let label = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
        label.filter(|label| !label.is_empty())
    }
}
