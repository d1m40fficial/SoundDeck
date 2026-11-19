use std::fs;
use std::path::PathBuf;

const DEFAULT_ENABLED: bool = true;

pub struct Settings {
    /// AUMID of the app the user pinned in the popup, if any.
    pub pinned: Option<String>,
    /// Whether the strip is shown on the taskbar.
    pub enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pinned: None,
            enabled: DEFAULT_ENABLED,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("SoundDeck").join("settings.txt"))
}

impl Settings {
    pub fn load() -> Self {
        let mut settings = Settings::default();
        let Some(path) = settings_path() else {
            return settings;
        };
        let Ok(text) = fs::read_to_string(path) else {
            return settings;
        };

        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "pinned" => {
                    let value = value.trim();
                    settings.pinned = (!value.is_empty()).then(|| value.to_string());
                }
                "enabled" => settings.enabled = value.trim() != "0",
                _ => {}
            }
        }
        settings
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut text = String::new();
        if let Some(pinned) = &self.pinned {
            // Keep the file one-line-per-key even if an id ever contains odd whitespace.
            if !pinned.contains(['\n', '\r']) {
                text.push_str(&format!("pinned={pinned}\n"));
            }
        }
        text.push_str(&format!("enabled={}\n", if self.enabled { 1 } else { 0 }));
        let _ = fs::write(path, text);
    }
}
