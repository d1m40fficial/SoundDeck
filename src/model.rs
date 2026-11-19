use std::sync::Arc;

/// Playback state reduced to what the widget displays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Playback {
    Playing,
    Paused,
    Stopped,
}

impl Playback {
    pub fn is_playing(self) -> bool {
        matches!(self, Playback::Playing)
    }

    pub fn label(self) -> &'static str {
        match self {
            Playback::Playing => "Playing",
            Playback::Paused => "Paused",
            Playback::Stopped => "Stopped",
        }
    }
}

/// Decoded album artwork, already converted to BGRA for `StretchDIBits`.
#[derive(Clone)]
pub struct Artwork {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

/// One media session exposed by the Windows media transport controls.
#[derive(Clone)]
pub struct Track {
    /// Unique per session: `AUMID|ordinal`. Several tabs of one browser share
    /// the AUMID, so the ordinal keeps them apart.
    pub key: String,
    pub app_id: String,
    pub app_name: String,
    pub title: String,
    pub artist: String,

    pub playback: Playback,
    pub can_toggle: bool,
    pub can_next: bool,
    pub can_prev: bool,
    pub art: Option<Artwork>,
}

impl Track {
    /// Identifies the displayed content; used to reset the marquee and artwork.
    pub fn content_key(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{}", self.key, self.title, self.artist)
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub tracks: Vec<Track>,
    /// App id of the session Windows considers current, if any.
    pub current: Option<String>,
}

impl Snapshot {
    /// Index of a pinned session (`AUMID|ordinal`).
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.key == key)
    }

    /// Index of the best session for an app id: a playing one if possible.
    pub fn index_of_app(&self, app_id: &str) -> Option<usize> {
        let mut fallback = None;
        for (index, track) in self.tracks.iter().enumerate() {
            if track.app_id != app_id {
                continue;
            }
            if track.playback.is_playing() {
                return Some(index);
            }
            fallback.get_or_insert(index);
        }
        fallback
    }
}
