//! macOS: process activities (no App Nap, no display sleep) and QoS classes.

use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

/// An activity begun with `NSProcessInfo`, ended when dropped.
pub struct Hold(Retained<ProtocolObject<dyn NSObjectProtocol>>);

/// User-initiated and latency-critical: no App Nap, no idle sleep, timers on time.
pub fn begin() -> Hold {
    activity(NSActivityOptions::UserInteractive, "Playing video")
}

pub fn screen() -> Hold {
    activity(NSActivityOptions::IdleDisplaySleepDisabled, "Showing video")
}

/// User-initiated: no idle sleep, no App Nap slowing the reading down.
pub fn recording() -> Hold {
    activity(NSActivityOptions::UserInitiated, "Recording")
}

fn activity(options: NSActivityOptions, reason: &str) -> Hold {
    Hold(NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &NSString::from_str(reason)))
}

impl Drop for Hold {
    fn drop(&mut self) {
        // SAFETY: the token came from beginActivityWithOptions:reason: and is ended once.
        unsafe { NSProcessInfo::processInfo().endActivity(&self.0) };
    }
}

pub fn high_priority_thread() {
    // SAFETY: plain libc call on the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
