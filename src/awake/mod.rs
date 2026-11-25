//! Telling the OS that haze is busy while it plays.
//!
//! - The display mustn't dim or sleep under a playing picture: [`Awake::screen`], on
//!   every platform.
//! - The computer mustn't go to sleep while a channel is recorded, even with no one
//!   watching: [`Awake::recording`].
//! - Audio for an AirPlay speaker goes out over the network, not the sound card, so
//!   macOS can't see that music is playing: with the window in the background it puts
//!   haze in App Nap and runs its threads at background priority, and under load the
//!   speaker runs dry. [`Awake::begin`] and [`high_priority_thread`] keep it from that;
//!   the other platforms don't throttle like that, so there they do nothing.

#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(all(unix, not(target_os = "macos")), path = "linux.rs")]
mod platform;

/// Held while it's needed; dropping it lets the OS dim, sleep or throttle again.
pub struct Awake {
    _hold: platform::Hold,
}

impl Awake {
    /// Keeps haze from being throttled while it plays.
    pub fn begin() -> Self {
        Awake { _hold: platform::begin() }
    }

    /// Keeps the display from dimming and going to sleep while a picture plays.
    pub fn screen() -> Self {
        Awake { _hold: platform::screen() }
    }

    /// Keeps the computer from going to sleep (the display may) while it records.
    pub fn recording() -> Self {
        Awake { _hold: platform::recording() }
    }
}

/// Run the calling thread at the priority the OS gives interactive work, for threads
/// that keep the speaker fed.
pub fn high_priority_thread() {
    platform::high_priority_thread();
}
