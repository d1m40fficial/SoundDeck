use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus, MediaPropertiesChangedEventArgs,
    PlaybackInfoChangedEventArgs, SessionsChangedEventArgs,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::icons;
use crate::model::{Artwork, Playback, Snapshot, Track};

pub const WM_SESSIONS: u32 = WM_APP + 10;

pub enum Command {
    Toggle(String),
    Next(String),
    Previous(String),
    Refresh,
}

struct Watched {
    session: GlobalSystemMediaTransportControlsSession,
    media_token: Option<i64>,
    state_token: Option<i64>,
}

/// Starts the WinRT media-session watcher on its own MTA thread.
///
/// The thread owns every WinRT object and pushes fresh snapshots into `shared`
/// before posting `WM_SESSIONS` to the UI thread.
pub fn start(hwnd: HWND, shared: Arc<Mutex<Snapshot>>) -> Sender<Command> {
    let (tx, rx) = mpsc::channel();
    let thread_tx = tx.clone();
    let raw_hwnd = hwnd.0 as isize;
    thread::spawn(move || run(raw_hwnd, shared, rx, thread_tx));
    tx
}

fn run(raw_hwnd: isize, shared: Arc<Mutex<Snapshot>>, rx: Receiver<Command>, tx: Sender<Command>) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    let Some(manager) = request_manager() else {
        return;
    };

    // Wake the worker as soon as Windows reports any session change.
    let sessions_tx = tx.clone();
    let handler = TypedEventHandler::<
        GlobalSystemMediaTransportControlsSessionManager,
        SessionsChangedEventArgs,
    >::new(move |_, _| {
        let _ = sessions_tx.send(Command::Refresh);
        Ok(())
    });
    let _ = manager.SessionsChanged(&handler);

    let current_tx = tx.clone();
    let handler = TypedEventHandler::<
        GlobalSystemMediaTransportControlsSessionManager,
        CurrentSessionChangedEventArgs,
    >::new(move |_, _| {
        let _ = current_tx.send(Command::Refresh);
        Ok(())
    });
    let _ = manager.CurrentSessionChanged(&handler);

    let mut watches: HashMap<String, Watched> = HashMap::new();
    let mut artwork: HashMap<String, (String, Option<Artwork>)> = HashMap::new();
    let mut first = true;

    loop {
        let wait = if first {
            Duration::ZERO
        } else {
            Duration::from_secs(2)
        };
        first = false;
        match rx.recv_timeout(wait) {
            Ok(Command::Toggle(app)) => control(&watches, &app, Control::Toggle),
            Ok(Command::Next(app)) => control(&watches, &app, Control::Next),
            Ok(Command::Previous(app)) => control(&watches, &app, Control::Previous),
            Ok(Command::Refresh) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Coalesce a burst of change notifications into one snapshot.
        while let Ok(command) = rx.try_recv() {
            match command {
                Command::Toggle(app) => control(&watches, &app, Control::Toggle),
                Command::Next(app) => control(&watches, &app, Control::Next),
                Command::Previous(app) => control(&watches, &app, Control::Previous),
                Command::Refresh => {}
            }
        }

        let snapshot = read(&manager, &mut watches, &mut artwork, Some(&tx));
        if let Ok(mut slot) = shared.lock() {
            *slot = snapshot;
        }
        let hwnd = HWND(raw_hwnd as *mut core::ffi::c_void);
        let _ = unsafe { PostMessageW(Some(hwnd), WM_SESSIONS, WPARAM(0), LPARAM(0)) };
    }
}

fn request_manager() -> Option<GlobalSystemMediaTransportControlsSessionManager> {
    for _ in 0..40 {
        if let Ok(manager) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .and_then(|op| op.join())
        {
            return Some(manager);
        }
        thread::sleep(Duration::from_millis(500));
    }
    None
}

enum Control {
    Toggle,
    Next,
    Previous,
}

fn control(watches: &HashMap<String, Watched>, app: &str, action: Control) {
    let Some(watched) = watches.get(app) else {
        return;
    };
    let result = match action {
        Control::Toggle => watched
            .session
            .TryTogglePlayPauseAsync()
            .and_then(|op| op.join()),
        Control::Next => watched.session.TrySkipNextAsync().and_then(|op| op.join()),
        Control::Previous => watched
            .session
            .TrySkipPreviousAsync()
            .and_then(|op| op.join()),
    };
    if let Err(error) = result {
        eprintln!("SoundDeck: media command failed: {error}");
    }
}

fn read(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    watches: &mut HashMap<String, Watched>,
    artwork: &mut HashMap<String, (String, Option<Artwork>)>,
    tx: Option<&Sender<Command>>,
) -> Snapshot {
    let mut snapshot = Snapshot::default();

    let Ok(sessions) = manager.GetSessions() else {
        return snapshot;
    };
    let Ok(count) = sessions.Size() else {
        return snapshot;
    };

    let mut ordinals: HashMap<String, usize> = HashMap::new();
    let mut live_keys: HashSet<String> = HashSet::new();
    for index in 0..count {
        let Ok(session) = sessions.GetAt(index) else {
            continue;
        };
        let Ok(app_id) = session.SourceAppUserModelId().map(|id| id.to_string()) else {
            continue;
        };
        if app_id.is_empty() {
            continue;
        }

        // Browsers publish one SMTC session per playing tab, all with the same
        // AUMID; number them so every tab is listed and controllable.
        let ordinal = ordinals.entry(app_id.clone()).or_insert(0);
        let key = format!("{app_id}|{ordinal}");
        *ordinal += 1;
        live_keys.insert(key.clone());

        let info = session.GetPlaybackInfo().ok();
        let playback = info
            .as_ref()
            .and_then(|info| info.PlaybackStatus().ok())
            .map(|status| {
                if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
                    Playback::Playing
                } else if status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused
                {
                    Playback::Paused
                } else {
                    Playback::Stopped
                }
            })
            .unwrap_or(Playback::Stopped);

        let controls = info.as_ref().and_then(|info| info.Controls().ok());
        let can_toggle = controls
            .as_ref()
            .map(|c| c.IsPlayPauseToggleEnabled().unwrap_or(false))
            .unwrap_or(false);
        let can_next = controls
            .as_ref()
            .map(|c| c.IsNextEnabled().unwrap_or(false))
            .unwrap_or(false);
        let can_prev = controls
            .as_ref()
            .map(|c| c.IsPreviousEnabled().unwrap_or(false))
            .unwrap_or(false);

        let mut title = String::new();
        let mut artist = String::new();

        let mut thumbnail: Option<IRandomAccessStreamReference> = None;
        if let Ok(properties) = session
            .TryGetMediaPropertiesAsync()
            .and_then(|op| op.join())
        {
            title = properties
                .Title()
                .map(|value| value.to_string())
                .unwrap_or_default();
            artist = properties
                .Artist()
                .map(|value| value.to_string())
                .unwrap_or_default();

            thumbnail = properties.Thumbnail().ok();
        }

        let cache_key = format!("{title}\u{1f}{artist}");
        let art = match artwork.get(&key) {
            Some((cached, art)) if *cached == cache_key => art.clone(),
            _ => {
                let art = thumbnail
                    .as_ref()
                    .and_then(decode_artwork)
                    .filter(|_| !title.is_empty());
                artwork.insert(key.clone(), (cache_key, art.clone()));
                art
            }
        };

        snapshot.tracks.push(Track {
            key: key.clone(),
            app_name: icons::app_label(&app_id),
            app_id: app_id.clone(),
            title,
            artist,

            playback,
            can_toggle,
            can_next,
            can_prev,
            art,
        });

        if let Some(tx) = tx {
            if !watches.contains_key(&key) {
                watches.insert(key, watch(session, tx));
            }
        }
    }

    watches.retain(|key, watched| {
        let alive = live_keys.contains(key);
        if !alive {
            if let Some(token) = watched.media_token {
                let _ = watched.session.RemoveMediaPropertiesChanged(token);
            }
            if let Some(token) = watched.state_token {
                let _ = watched.session.RemovePlaybackInfoChanged(token);
            }
        }
        alive
    });
    artwork.retain(|key, _| live_keys.contains(key));

    snapshot.current = manager.GetCurrentSession().ok().and_then(|session| {
        session
            .SourceAppUserModelId()
            .ok()
            .map(|id| id.to_string())
            .filter(|id| !id.is_empty())
    });

    snapshot
}

fn watch(session: GlobalSystemMediaTransportControlsSession, tx: &Sender<Command>) -> Watched {
    let media_tx = tx.clone();
    let media = TypedEventHandler::<
        GlobalSystemMediaTransportControlsSession,
        MediaPropertiesChangedEventArgs,
    >::new(move |_, _| {
        let _ = media_tx.send(Command::Refresh);
        Ok(())
    });
    let media_token = session.MediaPropertiesChanged(&media).ok();

    let state_tx = tx.clone();
    let state = TypedEventHandler::<
        GlobalSystemMediaTransportControlsSession,
        PlaybackInfoChangedEventArgs,
    >::new(move |_, _| {
        let _ = state_tx.send(Command::Refresh);
        Ok(())
    });
    let state_token = session.PlaybackInfoChanged(&state).ok();

    Watched {
        session,
        media_token,
        state_token,
    }
}

fn decode_artwork(reference: &IRandomAccessStreamReference) -> Option<Artwork> {
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as usize;
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size];
    reader.ReadBytes(&mut bytes).ok()?;

    let image = image::load_from_memory(&bytes)
        .ok()?
        .thumbnail(256, 256)
        .to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for pixel in image.pixels() {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Some(Artwork {
        width,
        height,
        bgra: Arc::new(bgra),
    })
}

/// `--dump` diagnostic: prints every SMTC session SoundDeck can see.
pub fn dump() {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let Some(manager) = request_manager() else {
        println!("SMTC manager unavailable");
        return;
    };
    if let Ok(sessions) = manager.GetSessions() {
        println!("raw sessions: {}", sessions.Size().unwrap_or(0));
        for index in 0..sessions.Size().unwrap_or(0) {
            if let Ok(session) = sessions.GetAt(index) {
                println!(
                    "  id={:?} status={:?}",
                    session.SourceAppUserModelId().map(|id| id.to_string()),
                    session
                        .GetPlaybackInfo()
                        .and_then(|info| info.PlaybackStatus())
                );
            }
        }
    }
    let mut watches = HashMap::new();
    let mut artwork = HashMap::new();
    let snapshot = read(&manager, &mut watches, &mut artwork, None);
    if snapshot.tracks.is_empty() {
        println!("No media sessions.");
    }
    for track in &snapshot.tracks {
        println!(
            "{:<28} | {:?} | {} — {} | art: {}",
            track.app_name,
            track.playback,
            track.title,
            track.artist,
            track.art.is_some()
        );
        println!("  {}", track.app_id);
    }
    println!("current: {:?}", snapshot.current);
}
