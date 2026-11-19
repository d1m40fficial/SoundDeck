#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod config;
mod gfx;
mod icons;
mod model;
mod smtc;
mod theme;
mod ui;

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use config::Settings;
use gfx::Canvas;
use model::{Playback, Snapshot, Track};
use theme::px;
use ui::{Hot, PopupLayout, StripLayout};
use windows::Win32::Foundation::{
    COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, EndPaint, GetDC, GetMonitorInfoW, GetPixel, HBRUSH,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow, PAINTSTRUCT,
    ReleaseDC, ScreenToClient,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DROPSHADOW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowExW,
    FindWindowW, GWL_STYLE, GetClientRect, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    GetWindowRect, HICON, HMENU, HWND_TOP, HWND_TOPMOST, IDC_ARROW, IMAGE_FLAGS, IsWindow,
    KillTimer, LoadCursorW, MA_NOACTIVATE, MENU_ITEM_FLAGS, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    MSG, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetForegroundWindow, SetParent, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_NULL, WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASS_STYLES, WNDCLASSW,
    WNDPROC, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use smtc::Command;

const WM_TRAY: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_WATCHDOG: usize = 1;
const TIMER_MARQUEE: usize = 2;
const TIMER_HIDE: usize = 3;

const ID_TOGGLE: usize = 2001;
const ID_AUTO: usize = 2002;
const ID_CLEAR_PIN: usize = 2003;
const ID_EXIT: usize = 2004;
const ID_AUTOSTART: usize = 2005;

struct App {
    host: HWND,
    strip: Option<HWND>,
    popup: Option<HWND>,
    taskbar: HWND,
    snapshot: Snapshot,
    shared: Arc<Mutex<Snapshot>>,
    cmd: Sender<Command>,
    settings: Settings,
    display: Option<usize>,
    dpi: u32,
    fonts: Option<ui::Fonts>,
    theme: theme::Theme,
    strip_layout: Option<StripLayout>,
    hot: Hot,
    hover_row: Option<usize>,
    popup_visible: bool,
    popup_at_cursor: bool,
    suppress_hover: bool,
    hide_deadline: Option<Instant>,
    marquee_phase: f32,
    marquee_excess: i32,
    last_key: String,
    optimistic: HashMap<String, (Playback, Instant)>,
    tray_icon: HICON,
    tray_tip: String,
    last_geom: (i32, i32, i32, i32, bool),
    last_size: (i32, i32),
    last_theme_check: Instant,
}

// Everything in `App` is only touched by the UI thread (through the global
// mutex); Win32 handles are plain pointers, so tell the compiler it is fine.
unsafe impl Send for App {}

static APP: OnceLock<Mutex<App>> = OnceLock::new();

fn app() -> std::sync::MutexGuard<'static, App> {
    APP.get()
        .expect("SoundDeck state")
        .lock()
        .expect("SoundDeck state lock")
}

fn main() {
    if std::env::args().any(|arg| arg == "--dump") {
        smtc::dump();
        return;
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let instance: windows::Win32::Foundation::HINSTANCE = unsafe { GetModuleHandleW(None) }
        .expect("module handle")
        .into();

    // The taskbar widget and its tray icon are a singleton.
    unsafe {
        let _mutex = CreateMutexW(None, false, w!("SoundDeck.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
    }

    if !register_classes(instance) {
        eprintln!("SoundDeck: cannot register window classes");
        return;
    }

    let host = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("SoundDeckHost"),
            w!("SoundDeck"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    } {
        Ok(host) => host,
        Err(error) => {
            eprintln!("SoundDeck: cannot create host window: {error}");
            return;
        }
    };

    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let cmd = smtc::start(host, shared.clone());

    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let settings = Settings::load();
    autostart::apply(settings.autostart);
    let state = App {
        host,
        strip: None,
        popup: None,
        taskbar: HWND::default(),
        snapshot: Snapshot::default(),
        shared,
        cmd,
        settings,
        display: None,
        dpi,
        fonts: Some(ui::Fonts::new(dpi)),
        theme: theme::Theme::load(),
        strip_layout: None,
        hot: Hot::None,
        hover_row: None,
        popup_visible: false,
        popup_at_cursor: false,
        suppress_hover: false,
        hide_deadline: None,
        marquee_phase: 0.0,
        marquee_excess: 0,
        last_key: String::new(),
        optimistic: HashMap::new(),
        tray_icon: load_tray_icon(),
        tray_tip: String::new(),
        last_geom: (0, 0, 0, 0, false),
        last_size: (0, 0),
        last_theme_check: Instant::now(),
    };
    let _ = APP.set(Mutex::new(state));

    {
        let mut state = app();
        add_tray_icon(&mut state);
        ensure_strip(&mut state);
        update_tray_tip(&mut state);
        unsafe {
            SetTimer(Some(host), TIMER_WATCHDOG, 1000, None);
            SetTimer(Some(host), TIMER_MARQUEE, 40, None);
        }
    }

    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn register_classes(instance: windows::Win32::Foundation::HINSTANCE) -> bool {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let classes: [(&str, WNDPROC, WNDCLASS_STYLES); 3] = [
        ("SoundDeckHost", Some(host_proc), WNDCLASS_STYLES(0)),
        ("SoundDeckStrip", Some(strip_proc), WNDCLASS_STYLES(0)),
        ("SoundDeckPopup", Some(popup_proc), CS_DROPSHADOW),
    ];
    for (name, proc_, style) in classes {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let class = WNDCLASSW {
            style,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: HICON::default(),
            hCursor: cursor,
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(name.as_ptr()),
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return false;
        }
    }
    true
}

fn load_tray_icon() -> HICON {
    const PNG: &[u8] = include_bytes!("../assets/icon.png");
    unsafe {
        CreateIconFromResourceEx(PNG, true, 0x0003_0000, 32, 32, IMAGE_FLAGS(0)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Taskbar strip
// ---------------------------------------------------------------------------

fn taskbar() -> Option<HWND> {
    unsafe {
        let bar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        FindWindowExW(Some(bar), None, w!("TrayNotifyWnd"), None).ok()?;
        Some(bar)
    }
}

fn ensure_strip(state: &mut App) {
    let Some(bar) = taskbar() else {
        destroy_strip(state);
        state.taskbar = HWND::default();
        return;
    };

    if let Some(strip) = state.strip {
        if !unsafe { IsWindow(Some(strip)) }.as_bool() {
            destroy_strip(state);
        }
    }

    if state.taskbar != bar {
        destroy_strip(state);
        state.taskbar = bar;
    }

    let dpi = {
        let dpi = unsafe { GetDpiForWindow(bar) };
        if dpi == 0 { 96 } else { dpi }
    };
    if dpi != state.dpi {
        state.dpi = dpi;
        state.fonts = Some(ui::Fonts::new(dpi));
        state.last_size = (0, 0);
    }

    if state.strip.is_none() {
        let instance: windows::Win32::Foundation::HINSTANCE = unsafe { GetModuleHandleW(None) }
            .expect("module handle")
            .into();
        // CreateWindowExW refuses a cross-process parent for a new child window,
        // but SetParent into the taskbar is allowed (this is how taskbar widgets do it).
        let strip = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
                w!("SoundDeckStrip"),
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
        };
        let strip = match strip {
            Ok(strip) => strip,
            Err(_) => return,
        };
        unsafe {
            let _ = SetParent(strip, Some(bar));
            let style = GetWindowLongPtrW(strip, GWL_STYLE);
            let wanted = style | WS_CHILD.0 as isize | WS_CLIPSIBLINGS.0 as isize;
            SetWindowLongPtrW(strip, GWL_STYLE, wanted & !(WS_POPUP.0 as isize));
        }
        state.strip = Some(strip);
        state.last_geom = (0, 0, 0, 0, false);
    }

    let Some(track) = state
        .display
        .and_then(|index| state.snapshot.tracks.get(index))
        .cloned()
    else {
        hide_strip(state);
        return;
    };

    let bar_rect = rect_of(bar).unwrap_or_default();
    if bar_rect.bottom - bar_rect.top > bar_rect.right - bar_rect.left {
        // A vertical taskbar has no room for a horizontal strip; use the tray menu.
        hide_strip(state);
        return;
    }

    let Ok(notify) = (unsafe { FindWindowExW(Some(bar), None, w!("TrayNotifyWnd"), None) }) else {
        hide_strip(state);
        return;
    };
    let notify_rect = rect_of(notify).unwrap_or_default();
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(bar, &mut client) };
    let bar_height = client.bottom - client.top;

    let Some(fonts) = state.fonts.as_ref() else {
        return;
    };
    let layout = ui::strip_layout(dpi, fonts, &track, bar_height);
    let title = if track.title.is_empty() {
        &track.app_name
    } else {
        &track.title
    };
    state.marquee_excess =
        (ui::measure(fonts.strip_title, title) - (layout.title.right - layout.title.left)).max(0);
    state.strip_layout = Some(layout.clone());

    // Sit immediately left of the clock, past the weather/widgets button when present.
    let mut anchor = notify_rect.left;
    for class in [w!("ShellFeedsTaskbarViewWindow"), w!("DynamicContent2")] {
        if let Ok(feeds) = unsafe { FindWindowExW(Some(bar), None, class, None) } {
            if let Some(feeds_rect) = rect_of(feeds) {
                // Only the weather widget that sits directly left of the clock
                // matters. Windows 11 also has DynamicContent windows at the far
                // left of the taskbar; those must not move our anchor.
                let adjacent = feeds_rect.right > notify_rect.left - px(160, dpi);
                if adjacent && feeds_rect.left < anchor && feeds_rect.right > feeds_rect.left {
                    anchor = feeds_rect.left;
                }
            }
        }
    }

    let mut origin = POINT {
        x: anchor,
        y: bar_rect.top,
    };
    let _ = unsafe { ScreenToClient(bar, &mut origin) };
    let margin = px(2, dpi);
    let x = origin.x - layout.width - margin;
    let y = ((bar_height - layout.height) / 2).max(0);

    let geometry = (x, y, layout.width, layout.height, true);
    state.last_geom = geometry;

    // Match the real taskbar shade (custom colors, transparency, accent tint).
    let mut probe = POINT { x, y };
    let _ = unsafe { ClientToScreen(bar, &mut probe) };
    let gap_x = probe.x - px(4, dpi);
    if let Some(shade) = sample_taskbar_color(bar_rect, gap_x, (bar_rect.top + bar_rect.bottom) / 2)
    {
        if shade != state.theme.chip {
            state.theme.set_chip(shade);
            invalidate(state.strip);
            invalidate(state.popup);
        }
    }
    {
        let strip = state.strip.expect("strip");
        // Explorer's composition surface is raised above our child on its layout
        // passes; without a periodic HWND_TOP the taskbar would eat mouse input.
        unsafe {
            let _ = SetWindowPos(
                strip,
                Some(HWND_TOP),
                x,
                y,
                layout.width,
                layout.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }
    if state.last_size != (layout.width, layout.height) {
        state.last_size = (layout.width, layout.height);
    }
    unsafe {
        let _ = ShowWindow(state.strip.expect("strip"), SW_SHOWNOACTIVATE);
    }
}

fn destroy_strip(state: &mut App) {
    if let Some(strip) = state.strip.take() {
        if unsafe { IsWindow(Some(strip)) }.as_bool() {
            let _ = unsafe { DestroyWindow(strip) };
        }
    }
    state.strip_layout = None;
    state.last_geom = (0, 0, 0, 0, false);
    state.last_size = (0, 0);
}

fn hide_strip(state: &mut App) {
    if let Some(strip) = state.strip {
        if state.last_geom.4 {
            state.last_geom = (0, 0, 0, 0, false);
            unsafe {
                let _ = ShowWindow(strip, SW_HIDE);
            }
        }
    }
    if state.popup_visible {
        hide_popup(state);
    }
}

fn rect_of(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

/// Samples the taskbar background just left of the widget and takes the median
/// of a small vertical band, so a nearby icon cannot shift the whole palette.
fn sample_taskbar_color(bar: RECT, x: i32, y: i32) -> Option<COLORREF> {
    if x <= bar.left || x >= bar.right || y <= bar.top || y >= bar.bottom {
        return None;
    }
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        return None;
    }

    let mut red = Vec::new();
    let mut green = Vec::new();
    let mut blue = Vec::new();
    for dy in [-6, 0, 6] {
        for dx in [-1, -4, -7] {
            let color = unsafe { GetPixel(dc, x + dx, y + dy) };
            if color.0 != u32::MAX {
                red.push((color.0 & 0xFF) as u8);
                green.push(((color.0 >> 8) & 0xFF) as u8);
                blue.push(((color.0 >> 16) & 0xFF) as u8);
            }
        }
    }
    unsafe {
        let _ = ReleaseDC(None, dc);
    }
    if red.is_empty() {
        return None;
    }

    let median = |values: &mut Vec<u8>| {
        values.sort_unstable();
        values[values.len() / 2]
    };
    let (r, g, b) = (median(&mut red), median(&mut green), median(&mut blue));
    // Strongly saturated samples are icons or wallpaper, not the taskbar shade.
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min > 70 {
        return None;
    }
    Some(theme::rgb(r, g, b))
}

// ---------------------------------------------------------------------------
// Popup
// ---------------------------------------------------------------------------

fn show_popup(state: &mut App, at_cursor: bool) {
    if state.snapshot.tracks.is_empty() {
        hide_popup(state);
        return;
    }
    let count = state.snapshot.tracks.len().min(ui::MAX_ROWS);
    let layout = ui::popup_layout(state.dpi, count);

    if state.popup.is_none() {
        let instance: windows::Win32::Foundation::HINSTANCE = unsafe { GetModuleHandleW(None) }
            .expect("module handle")
            .into();
        let popup = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("SoundDeckPopup"),
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(state.host),
                None,
                Some(instance),
                None,
            )
        };
        let Ok(popup) = popup else { return };
        state.popup = Some(popup);
    }

    let popup = state.popup.expect("popup");
    let (x, y) = popup_position(state, &layout, at_cursor);

    unsafe {
        let _ = SetWindowPos(
            popup,
            Some(HWND_TOPMOST),
            x,
            y,
            layout.width,
            layout.height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    state.popup_visible = true;
    state.popup_at_cursor = at_cursor;
    invalidate(Some(popup));
}

fn popup_position(state: &App, layout: &PopupLayout, at_cursor: bool) -> (i32, i32) {
    let mut work = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    let mut anchor_rect = None;

    if at_cursor || state.strip.is_none() || !state.last_geom.4 {
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
            anchor_rect = Some(RECT {
                left: cursor.x,
                top: cursor.y,
                right: cursor.x,
                bottom: cursor.y,
            });
            if let Some(info) =
                monitor_info(unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) })
            {
                work = info.rcWork;
            }
        }
    } else if let Some(strip) = state.strip {
        if let Some(rect) = rect_of(strip) {
            anchor_rect = Some(rect);
            if let Some(info) =
                monitor_info(unsafe { MonitorFromWindow(strip, MONITOR_DEFAULTTONEAREST) })
            {
                work = info.rcWork;
            }
        }
    }

    let anchor = anchor_rect.unwrap_or_default();
    let gap = px(8, state.dpi);
    let pad = px(4, state.dpi);
    let mut x = anchor.right - layout.width;
    let mut y = anchor.top - layout.height - gap;
    if y < work.top + pad {
        y = anchor.bottom + gap;
    }
    x = x.clamp(
        work.left + pad,
        (work.right - layout.width - pad).max(work.left),
    );
    y = y.clamp(
        work.top + pad,
        (work.bottom - layout.height - pad).max(work.top),
    );
    (x, y)
}

fn monitor_info(monitor: windows::Win32::Graphics::Gdi::HMONITOR) -> Option<MONITORINFO> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then_some(info)
}

fn hide_popup(state: &mut App) {
    if let Some(popup) = state.popup {
        unsafe {
            let _ = ShowWindow(popup, SW_HIDE);
        }
    }
    state.popup_visible = false;
    state.hover_row = None;
    state.hide_deadline = None;
    kill_timer(Some(state.host), TIMER_HIDE);
}

fn schedule_hide(state: &mut App) {
    if !state.popup_visible {
        return;
    }
    state.hide_deadline = Some(Instant::now() + Duration::from_millis(320));
    unsafe {
        SetTimer(Some(state.host), TIMER_HIDE, 320, None);
    }
}

// ---------------------------------------------------------------------------
// Snapshot wiring
// ---------------------------------------------------------------------------

fn refresh_from_shared(state: &mut App) {
    if let Ok(shared) = state.shared.lock() {
        state.snapshot = shared.clone();
    }
    apply_optimistic(state);
    recompute_display(state);

    let key = state
        .display
        .and_then(|index| state.snapshot.tracks.get(index))
        .map(Track::content_key)
        .unwrap_or_default();
    if key != state.last_key {
        state.last_key = key;
        state.marquee_phase = 0.0;
        state.hot = Hot::None;
        if state.popup_visible {
            state.hover_row = None;
        }
    }

    ensure_strip(state);
    if state.popup_visible {
        if state.snapshot.tracks.is_empty() {
            hide_popup(state);
        } else {
            show_popup(state, state.popup_at_cursor);
        }
    }
    update_tray_tip(state);
    invalidate(state.strip);
    if state.popup_visible {
        invalidate(state.popup);
    }
}

fn apply_optimistic(state: &mut App) {
    let mut settled = Vec::new();
    for track in &mut state.snapshot.tracks {
        let Some((desired, since)) = state.optimistic.get(&track.key).copied() else {
            continue;
        };
        if track.playback == desired || since.elapsed() >= Duration::from_millis(1500) {
            settled.push(track.key.clone());
        } else {
            track.playback = desired;
        }
    }
    for key in settled {
        state.optimistic.remove(&key);
    }
}

fn recompute_display(state: &mut App) {
    let index = state
        .settings
        .pinned
        .as_deref()
        .and_then(|key| state.snapshot.index_of(key))
        .or_else(|| {
            state
                .snapshot
                .current
                .as_deref()
                .and_then(|id| state.snapshot.index_of_app(id))
        })
        .or_else(|| {
            state
                .snapshot
                .tracks
                .iter()
                .position(|track| track.playback.is_playing())
        })
        .or_else(|| (!state.snapshot.tracks.is_empty()).then_some(0));
    state.display = index;
}

fn update_tray_tip(state: &mut App) {
    let tip = match state
        .display
        .and_then(|index| state.snapshot.tracks.get(index))
    {
        Some(track) => {
            let title = if track.title.is_empty() {
                &track.app_name
            } else {
                &track.title
            };
            if track.artist.is_empty() {
                format!("SoundDeck — {title}")
            } else {
                format!("SoundDeck — {title} — {}", track.artist)
            }
        }
        None => "SoundDeck — no active media".to_string(),
    };
    if tip == state.tray_tip {
        return;
    }
    state.tray_tip = tip;
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.host;
    data.uID = 1;
    data.uFlags = NIF_TIP;
    set_tip_text(&mut data, &state.tray_tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn set_tip_text(data: &mut NOTIFYICONDATAW, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().take(data.szTip.len() - 1).collect();
    for (slot, ch) in data.szTip.iter_mut().zip(wide.iter()) {
        *slot = *ch;
    }
    if let Some(tail) = data.szTip.get_mut(wide.len()) {
        *tail = 0;
    }
}

// ---------------------------------------------------------------------------
// Tray icon and menu
// ---------------------------------------------------------------------------

fn add_tray_icon(state: &mut App) {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.host;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = state.tray_icon;
    set_tip_text(&mut data, "SoundDeck");
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
}

fn remove_tray_icon(state: &App) {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = state.host;
    data.uID = 1;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
    if !state.tray_icon.is_invalid() {
        let _ = unsafe { DestroyIcon(state.tray_icon) };
    }
}

fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        let _ = AppendMenuW(menu, flags, id, PCWSTR(wide.as_ptr()));
    }
}

/// Modal menu. Must be called without holding the app lock: it pumps messages.
fn show_menu(host: HWND, enabled: bool, pinned: bool, autostart_enabled: bool) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        let toggle = if enabled {
            "Hide from taskbar"
        } else {
            "Show on taskbar"
        };
        append(menu, MF_STRING, ID_TOGGLE, toggle);
        let auto_flags = if pinned {
            MF_STRING
        } else {
            MF_STRING | MF_CHECKED
        };
        append(menu, auto_flags, ID_AUTO, "Auto-select source");
        if pinned {
            append(menu, MF_STRING, ID_CLEAR_PIN, "Unpin source");
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let autostart_flags = if autostart_enabled {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        append(menu, autostart_flags, ID_AUTOSTART, "Start with Windows");
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        append(menu, MF_STRING, ID_EXIT, "Exit");

        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let _ = SetForegroundWindow(host);
        let command = windows::Win32::UI::WindowsAndMessaging::TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            None,
            host,
            None,
        );
        let _ = PostMessageW(Some(host), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        command.0.max(0) as usize
    }
}

fn take_menu_snapshot() -> (HWND, bool, bool, bool) {
    let state = app();
    (
        state.host,
        state.settings.enabled,
        state.settings.pinned.is_some(),
        state.settings.autostart,
    )
}

fn handle_menu(state: &mut App, command: usize) {
    match command {
        ID_TOGGLE => {
            state.settings.enabled = !state.settings.enabled;
            state.settings.save();
            if state.settings.enabled {
                ensure_strip(state);
            } else {
                hide_strip(state);
            }
            invalidate(state.strip);
        }
        ID_AUTO | ID_CLEAR_PIN => {
            state.settings.pinned = None;
            state.settings.save();
            recompute_display(state);
            ensure_strip(state);
            invalidate(state.strip);
            invalidate(state.popup);
        }
        ID_AUTOSTART => {
            state.settings.autostart = !state.settings.autostart;
            state.settings.save();
            autostart::apply(state.settings.autostart);
        }
        ID_EXIT => unsafe {
            // DestroyWindow sends WM_DESTROY synchronously; defer it so we do
            // not re-enter the state lock.
            let _ = PostMessageW(Some(state.host), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        _ => {}
    }
}

fn select_track(state: &mut App, index: usize) {
    let Some(track) = state.snapshot.tracks.get(index) else {
        return;
    };
    state.settings.pinned = Some(track.key.clone());
    state.settings.save();
    recompute_display(state);
    ensure_strip(state);
    invalidate(state.strip);
    invalidate(state.popup);
}

// ---------------------------------------------------------------------------
// Window procedures
// ---------------------------------------------------------------------------

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER => {
            let id = wparam.0;
            let mut state = app();
            match id {
                TIMER_WATCHDOG => {
                    ensure_strip(&mut state);
                    if state.last_theme_check.elapsed() > Duration::from_secs(4) {
                        state.last_theme_check = Instant::now();
                        let theme = theme::Theme::load();
                        if theme.dark != state.theme.dark || theme.accent != state.theme.accent {
                            state.theme = theme;
                            invalidate(state.strip);
                            invalidate(state.popup);
                        }
                    }
                }
                TIMER_MARQUEE => {
                    if state.marquee_excess > 0 {
                        state.marquee_phase += 0.04;
                        invalidate(state.strip);
                    }
                }
                TIMER_HIDE => {
                    kill_timer(Some(hwnd), TIMER_HIDE);
                    if let Some(deadline) = state.hide_deadline {
                        if Instant::now() >= deadline && !cursor_inside(&state) {
                            hide_popup(&mut state);
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        smtc::WM_SESSIONS => {
            let mut state = app();

            refresh_from_shared(&mut state);
            LRESULT(0)
        }
        WM_TRAY => {
            let notification = (lparam.0 as u32) & 0xFFFF;
            match notification {
                WM_RBUTTONUP | WM_CONTEXTMENU => {
                    let (host, enabled, pinned, autostart_enabled) = take_menu_snapshot();
                    let command = show_menu(host, enabled, pinned, autostart_enabled);
                    handle_menu(&mut app(), command);
                }
                WM_LBUTTONUP => {
                    let mut state = app();
                    if state.popup_visible {
                        hide_popup(&mut state);
                    } else {
                        show_popup(&mut state, true);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            let mut state = app();
            state.theme = theme::Theme::load();
            state.dpi = unsafe { GetDpiForSystem() }.max(96);
            state.fonts = Some(ui::Fonts::new(state.dpi));
            state.last_size = (0, 0);
            ensure_strip(&mut state);
            invalidate(state.strip);
            invalidate(state.popup);
            LRESULT(0)
        }
        WM_DESTROY => {
            let state = app();
            remove_tray_icon(&state);
            kill_timer(Some(hwnd), TIMER_WATCHDOG);
            kill_timer(Some(hwnd), TIMER_MARQUEE);
            kill_timer(Some(hwnd), TIMER_HIDE);
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn strip_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut state = app();
            paint_strip_window(&mut state, hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut state = app();
            track_leave(hwnd);
            let layout = state.strip_layout.clone();
            if let Some(layout) = layout {
                let hot = layout.hit(low_word(lparam) as i32, high_word(lparam) as i32);
                if hot != state.hot {
                    state.hot = hot;
                    invalidate(Some(hwnd));
                }
            }
            if !state.popup_visible && !state.suppress_hover && !state.snapshot.tracks.is_empty() {
                show_popup(&mut state, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let mut state = app();

            if state.hot != Hot::None {
                state.hot = Hot::None;
                invalidate(Some(hwnd));
            }
            state.suppress_hover = false;
            schedule_hide(&mut state);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut state = app();
            let x = low_word(lparam) as i32;
            let y = high_word(lparam) as i32;
            let hot = state
                .strip_layout
                .as_ref()
                .map(|layout| layout.hit(x, y))
                .unwrap_or(Hot::None);
            let session_key = state
                .display
                .and_then(|index| state.snapshot.tracks.get(index))
                .map(|t| t.key.clone());
            let (can_prev, can_toggle, can_next) = state
                .display
                .and_then(|index| state.snapshot.tracks.get(index))
                .map(|t| (t.can_prev, t.can_toggle, t.can_next))
                .unwrap_or((false, false, false));
            match hot {
                Hot::Prev => {
                    if can_prev {
                        if let Some(key) = session_key {
                            let _ = state.cmd.send(Command::Previous(key));
                        }
                    }
                }
                Hot::Next => {
                    if can_next {
                        if let Some(key) = session_key {
                            let _ = state.cmd.send(Command::Next(key));
                        }
                    }
                }
                Hot::Toggle => {
                    if can_toggle {
                        if let Some(index) = state.display {
                            let (key, desired) = {
                                let track = &mut state.snapshot.tracks[index];
                                track.playback = if track.playback.is_playing() {
                                    Playback::Paused
                                } else {
                                    Playback::Playing
                                };
                                (track.key.clone(), track.playback)
                            };
                            // Flip the icon immediately and keep it until the
                            // session reports back (or the 1.5s guard expires).
                            state
                                .optimistic
                                .insert(key.clone(), (desired, Instant::now()));
                            invalidate(state.strip);
                            invalidate(state.popup);
                            let _ = state.cmd.send(Command::Toggle(key));
                        }
                    }
                }
                _ => {
                    if state.popup_visible {
                        hide_popup(&mut state);
                        state.suppress_hover = true;
                    } else {
                        show_popup(&mut state, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (host, enabled, pinned, autostart_enabled) = take_menu_snapshot();
            let command = show_menu(host, enabled, pinned, autostart_enabled);
            handle_menu(&mut app(), command);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut state = app();
            paint_popup_window(&mut state, hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut state = app();
            track_leave(hwnd);
            let count = state.snapshot.tracks.len().min(ui::MAX_ROWS);
            let layout = ui::popup_layout(state.dpi, count);
            let x = low_word(lparam) as i32;
            let y = high_word(lparam) as i32;
            let hover = layout
                .rows
                .iter()
                .position(|row| x >= row.left && x < row.right && y >= row.top && y < row.bottom);
            if hover != state.hover_row {
                state.hover_row = hover;
                invalidate(Some(hwnd));
            }
            state.hide_deadline = None;
            kill_timer(Some(state.host), TIMER_HIDE);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let mut state = app();

            if state.hover_row.take().is_some() {
                invalidate(Some(hwnd));
            }
            schedule_hide(&mut state);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut state = app();
            let x = low_word(lparam) as i32;
            let y = high_word(lparam) as i32;
            let count = state.snapshot.tracks.len().min(ui::MAX_ROWS);
            let layout = ui::popup_layout(state.dpi, count);
            if let Some(index) = layout
                .rows
                .iter()
                .position(|row| x >= row.left && x < row.right && y >= row.top && y < row.bottom)
            {
                select_track(&mut state, index);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (host, enabled, pinned, autostart_enabled) = take_menu_snapshot();
            let command = show_menu(host, enabled, pinned, autostart_enabled);
            handle_menu(&mut app(), command);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn track_leave(hwnd: HWND) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut event);
    }
}

fn cursor_inside(state: &App) -> bool {
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return false;
    }
    let inside = |hwnd: HWND| {
        rect_of(hwnd)
            .map(|rect| point_in(rect, cursor))
            .unwrap_or(false)
    };
    state.strip.map(inside).unwrap_or(false)
        || (state.popup_visible && state.popup.map(inside).unwrap_or(false))
}

fn point_in(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

fn paint_strip_window(state: &mut App, hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let _ = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };

    if let (Some(layout), Some(track), Some(fonts)) = (
        state.strip_layout.clone(),
        state
            .display
            .and_then(|index| state.snapshot.tracks.get(index))
            .cloned(),
        state.fonts.as_ref(),
    ) {
        if let Some(canvas) = Canvas::new(
            layout.width.max(client.right),
            layout.height.max(client.bottom),
        ) {
            let offset = marquee_offset(state);
            ui::paint_strip(
                &state.theme,
                fonts,
                &track,
                &layout,
                state.hot,
                offset,
                &canvas,
            );
            // Square corners: the strip must merge into the taskbar exactly.
            canvas.present(hwnd, 0);
        }
    }
    unsafe {
        let _ = EndPaint(hwnd, &paint);
    }
}

fn paint_popup_window(state: &mut App, hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let _ = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };

    let count = state.snapshot.tracks.len().min(ui::MAX_ROWS);
    let layout = ui::popup_layout(state.dpi, count);
    if let Some(canvas) = Canvas::new(
        layout.width.max(client.right),
        layout.height.max(client.bottom),
    ) {
        if let Some(fonts) = state.fonts.as_ref() {
            ui::paint_popup(
                &state.theme,
                fonts,
                &state.snapshot.tracks,
                state.display,
                state.hover_row,
                &layout,
                &canvas,
            );
            canvas.present(hwnd, px(10, layout.dpi));
        }
    }
    unsafe {
        let _ = EndPaint(hwnd, &paint);
    }
}

fn marquee_offset(state: &App) -> i32 {
    let excess = state.marquee_excess;
    if excess <= 0 {
        return 0;
    }
    let speed = 26.0;
    let pause = 1.4f32;
    let travel = excess as f32 / speed;
    let cycle = 2.0 * (pause + travel);
    let t = state.marquee_phase % cycle;
    if t < pause {
        0
    } else if t < pause + travel {
        ((t - pause) * speed) as i32
    } else if t < 2.0 * pause + travel {
        excess
    } else {
        (excess as f32 - (t - 2.0 * pause - travel) * speed).max(0.0) as i32
    }
}

fn invalidate(hwnd: Option<HWND>) {
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
        }
    }
}

fn kill_timer(hwnd: Option<HWND>, id: usize) {
    unsafe {
        let _ = KillTimer(hwnd, id);
    }
}

fn low_word(value: LPARAM) -> u16 {
    (value.0 as u32 & 0xFFFF) as u16
}

fn high_word(value: LPARAM) -> u16 {
    ((value.0 as u32 >> 16) & 0xFFFF) as u16
}
