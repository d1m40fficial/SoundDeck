// Development helper: plays a local WAV through WinRT MediaPlayer so it shows
// up as a System Media Transport Controls session for testing SoundDeck.
use std::thread;
use std::time::Duration;

use windows::Foundation::Uri;
use windows::Media::Core::MediaSource;
use windows::Media::MediaPlaybackType;
use windows::Media::Playback::MediaPlayer;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::HSTRING;

fn main() {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "C:/temp/opencode/test.wav".to_string());
    let path = path.replace('\\', "/");
    let uri = Uri::CreateUri(&HSTRING::from(format!(
        "file:///{}",
        path.trim_start_matches('/')
    )))
    .expect("uri");
    let source = MediaSource::CreateFromUri(&uri).expect("source");
    let player = MediaPlayer::new().expect("player");
    player.SetSource(&source).expect("source set");

    player.Play().expect("play");
    thread::sleep(Duration::from_millis(600));

    if let Ok(controls) = player.SystemMediaTransportControls() {
        let _ = controls.SetIsEnabled(true);
        if let Ok(updater) = controls.DisplayUpdater() {
            let _ = updater.SetType(MediaPlaybackType::Music);
            if let Ok(music) = updater.MusicProperties() {
                let _ = music.SetTitle(&HSTRING::from(
                    "SoundDeck Test Track — a deliberately long title so the marquee has something to scroll",
                ));
                let _ = music.SetArtist(&HSTRING::from("Test Artist"));
                let _ = music.SetAlbumTitle(&HSTRING::from("Test Album"));
            }
            let _ = updater.Update();
        }
    }

    println!("playing; ctrl-c to stop");
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
