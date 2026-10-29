//! Telling the OS that haze is busy while it plays. Audio for an AirPlay speaker goes
//! out over the network, not the sound card, so macOS can't see that music is playing:
//! with the window in the background it puts haze in App Nap and runs its threads at
//! background priority, and under load the speaker runs dry. The other platforms don't
//! throttle like that, so there this does nothing.

/// Held while playing; dropping it lets the OS throttle haze again.
pub struct Awake(#[cfg(target_os = "macos")] objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2::runtime::NSObjectProtocol>>);

impl Awake {
    pub fn begin() -> Self {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
            // User-initiated and latency-critical: no App Nap, no idle sleep, timers on time.
            let reason = NSString::from_str("Playing video");
            Awake(NSProcessInfo::processInfo().beginActivityWithOptions_reason(NSActivityOptions::UserInteractive, &reason))
        }
        #[cfg(not(target_os = "macos"))]
        Awake()
    }
}

#[cfg(target_os = "macos")]
impl Drop for Awake {
    fn drop(&mut self) {
        // SAFETY: the token came from beginActivityWithOptions:reason: and is ended once.
        unsafe { objc2_foundation::NSProcessInfo::processInfo().endActivity(&self.0) };
    }
}

/// Run the calling thread at the priority macOS gives interactive work, for threads
/// that keep the speaker fed. Elsewhere the default priority is fine.
pub fn high_priority_thread() {
    #[cfg(target_os = "macos")]
    // SAFETY: plain libc call on the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
