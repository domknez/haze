//! The OS media session: media keys, headset buttons and the "Now Playing" widget
//! (macOS Control Center, Windows media overlay, Linux MPRIS).

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig, SeekDirection};
use std::sync::mpsc::{Receiver, channel};

/// Seek step for "skip forward/back" buttons that don't say how far.
const SEEK_STEP: f64 = 10.0;

/// What a key press asks for.
pub enum Press {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Seek(f64),
    Volume(f32),
}

pub struct NowPlaying {
    controls: MediaControls,
    presses: Receiver<Press>,
    /// What was announced last: title, subtitle, logo and whether it plays.
    shown: Option<(String, String, Option<String>, bool)>,
}

impl NowPlaying {
    pub fn start(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let config = PlatformConfig { display_name: "haze", dbus_name: "haze", hwnd: window_handle(cc) };
        let mut controls = MediaControls::new(config).inspect_err(|e| log_warn!("media keys unavailable: {e:?}")).ok()?;
        let (tx, presses) = channel();
        let ctx = cc.egui_ctx.clone();
        controls
            .attach(move |event| {
                if let Some(p) = press(event) {
                    let _ = tx.send(p);
                    ctx.request_repaint();
                }
            })
            .inspect_err(|e| log_warn!("media keys unavailable: {e:?}"))
            .ok()?;
        Some(Self { controls, presses, shown: None })
    }

    pub fn presses(&self) -> Vec<Press> {
        self.presses.try_iter().collect()
    }

    /// Tell the OS what's on; cheap when nothing changed.
    pub fn update(&mut self, title: &str, subtitle: &str, logo: Option<&str>, playing: bool) {
        let now = (title.to_string(), subtitle.to_string(), logo.map(str::to_string), playing);
        if self.shown.as_ref() == Some(&now) {
            return;
        }
        let metadata = MediaMetadata { title: Some(title), artist: Some(subtitle), album: None, cover_url: logo, duration: None };
        if let Err(e) = self.controls.set_metadata(metadata) {
            log_debug!("now playing metadata: {e:?}");
        }
        let playback = if title.is_empty() {
            MediaPlayback::Stopped
        } else if playing {
            MediaPlayback::Playing { progress: None }
        } else {
            MediaPlayback::Paused { progress: None }
        };
        if let Err(e) = self.controls.set_playback(playback) {
            log_debug!("now playing state: {e:?}");
        }
        self.shown = Some(now);
    }
}

fn press(event: MediaControlEvent) -> Option<Press> {
    log_debug!("media key: {event:?}");
    let step = |dir| if dir == SeekDirection::Forward { 1.0 } else { -1.0 };
    Some(match event {
        MediaControlEvent::Play => Press::Play,
        MediaControlEvent::Pause | MediaControlEvent::Stop => Press::Pause,
        MediaControlEvent::Toggle => Press::Toggle,
        MediaControlEvent::Next => Press::Next,
        MediaControlEvent::Previous => Press::Prev,
        MediaControlEvent::Seek(dir) => Press::Seek(step(dir) * SEEK_STEP),
        MediaControlEvent::SeekBy(dir, by) => Press::Seek(step(dir) * by.as_secs_f64()),
        MediaControlEvent::SetVolume(v) => Press::Volume(v.clamp(0.0, 1.0) as f32),
        MediaControlEvent::SetPosition(_) | MediaControlEvent::OpenUri(_) | MediaControlEvent::Raise | MediaControlEvent::Quit => {
            return None;
        }
    })
}

/// Windows' media session is tied to a window.
#[cfg(windows)]
fn window_handle(cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(windows))]
fn window_handle(_: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    None
}
