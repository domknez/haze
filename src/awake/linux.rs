//! Linux and the BSDs: asking the desktop over D-Bus not to blank the screen. The
//! freedesktop screensaver interface covers KDE, Xfce and most others; GNOME's
//! session manager is asked when that isn't there. While recording, systemd-logind
//! is asked not to let the computer sleep.

use dbus::blocking::Connection;
use std::time::Duration;

/// How long to wait for the desktop's answer: this runs on the UI thread.
const TIMEOUT: Duration = Duration::from_millis(500);

/// The desktop's inhibition, if one was granted; lifted when dropped (and by the
/// desktop when the connection closes, should haze go away first).
pub struct Hold {
    _inhibition: Option<Inhibition>,
    /// logind's lock against sleep, held as long as this file stays open.
    _lock: Option<dbus::arg::OwnedFd>,
}

struct Inhibition {
    bus: Connection,
    via: Via,
    cookie: u32,
}

#[derive(Clone, Copy)]
enum Via {
    ScreenSaver,
    Gnome,
}

impl Via {
    /// (bus name, object path, interface, the method that lifts it)
    fn service(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Via::ScreenSaver => ("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver", "org.freedesktop.ScreenSaver", "UnInhibit"),
            Via::Gnome => ("org.gnome.SessionManager", "/org/gnome/SessionManager", "org.gnome.SessionManager", "Uninhibit"),
        }
    }

    fn inhibit(self, bus: &Connection) -> Result<u32, dbus::Error> {
        let (name, path, interface, _) = self.service();
        let proxy = bus.with_proxy(name, path, TIMEOUT);
        let (cookie,) = match self {
            Via::ScreenSaver => proxy.method_call(interface, "Inhibit", ("haze", "Showing video"))?,
            // No window id; 8 is "idle": the session doesn't go idle (blank, lock).
            Via::Gnome => proxy.method_call(interface, "Inhibit", ("haze", 0u32, "Showing video", 8u32))?,
        };
        Ok(cookie)
    }
}

/// Nothing throttles a player in the background here: nothing to hold.
pub fn begin() -> Hold {
    Hold { _inhibition: None, _lock: None }
}

pub fn screen() -> Hold {
    let bus = match Connection::new_session() {
        Ok(bus) => bus,
        Err(e) => {
            log_warn!("couldn't keep the display on: no session bus ({e})");
            return Hold { _inhibition: None, _lock: None };
        }
    };
    let mut failed = Vec::new();
    for via in [Via::ScreenSaver, Via::Gnome] {
        match via.inhibit(&bus) {
            Ok(cookie) => return Hold { _inhibition: Some(Inhibition { bus, via, cookie }), _lock: None },
            Err(e) => failed.push(e.message().unwrap_or("no answer").to_string()),
        }
    }
    log_warn!("couldn't keep the display on: {}", failed.join("; "));
    Hold { _inhibition: None, _lock: None }
}

/// logind's lock against sleep (and the idle action, often sleep), in its
/// "block" mode: lifted when the file it hands over is closed.
pub fn recording() -> Hold {
    let lock = Connection::new_system().and_then(|bus| {
        let logind = bus.with_proxy("org.freedesktop.login1", "/org/freedesktop/login1", TIMEOUT);
        let (fd,): (dbus::arg::OwnedFd,) =
            logind.method_call("org.freedesktop.login1.Manager", "Inhibit", ("sleep:idle", "haze", "Recording", "block"))?;
        Ok(fd)
    });
    if let Err(e) = &lock {
        log_warn!("couldn't keep the computer awake: {}", e.message().unwrap_or("no answer"));
    }
    Hold { _inhibition: None, _lock: lock.ok() }
}

impl Drop for Inhibition {
    fn drop(&mut self) {
        let (name, path, interface, lift) = self.via.service();
        let lifted: Result<(), _> = self.bus.with_proxy(name, path, TIMEOUT).method_call(interface, lift, (self.cookie,));
        if let Err(e) = lifted {
            log_warn!("couldn't let the display sleep again: {e}");
        }
    }
}

pub fn high_priority_thread() {}
