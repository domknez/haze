//! Minimal leveled logging to stderr, configured with `HAZE_LOG`
//! (`error`, `warn`, `info`, `debug`, `trace`; default `warn`).
//! `HAZE_DEBUG=1` is kept as a shorthand for `debug`.
//!
//! The app also keeps a log file at debug level (see [`to_file`]) so problems that
//! happen while nobody watches a terminal can be looked at afterwards.
//!
//! Use the macros: `log_error!`, `log_warn!`, `log_info!`, `log_debug!`, `log_trace!`.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// What goes into the log file.
const FILE_LEVEL: Level = Level::Debug;
/// The log is started over (the old one kept as `haze.log.1`) beyond this size.
const MAX_FILE_BYTES: u64 = 5 << 20;

static FILE: OnceLock<Mutex<File>> = OnceLock::new();

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn parse(s: &str) -> Option<Level> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "info" => Level::Info,
            "debug" => Level::Debug,
            "trace" => Level::Trace,
            _ => return None,
        })
    }
}

fn max_level() -> Level {
    static LEVEL: OnceLock<Level> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        let from_env = std::env::var("HAZE_LOG").ok().and_then(|v| Level::parse(&v));
        let legacy_debug = std::env::var_os("HAZE_DEBUG").is_some().then_some(Level::Debug);
        from_env.or(legacy_debug).unwrap_or(Level::Warn)
    })
}

pub fn enabled(level: Level) -> bool {
    level <= max_level() || (level <= FILE_LEVEL && FILE.get().is_some())
}

/// The log file: `~/Library/Logs/haze/haze.log` on macOS, `%LOCALAPPDATA%\haze\haze.log`
/// on Windows, `$XDG_STATE_HOME/haze/haze.log` (or `~/.local/state/...`) elsewhere.
pub fn file_path() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Logs/haze")
    } else if cfg!(windows) {
        env("LOCALAPPDATA")?.join("haze")
    } else {
        env("XDG_STATE_HOME").or_else(|| env("HOME").map(|h| h.join(".local/state")))?.join("haze")
    };
    Some(dir.join("haze.log"))
}

/// Start writing to the log file as well (the app does; command-line diagnostics don't).
pub fn to_file() {
    let Some(path) = file_path() else { return };
    match open(&path) {
        Ok(file) => {
            let _ = FILE.set(Mutex::new(file));
        }
        Err(e) => eprintln!("haze: no log file at {}: {e}", path.display()),
    }
}

/// Record panics in the log file (message, place and thread), then hand them on
/// to the usual handling. Without this a crash nobody saw in a terminal leaves no
/// trace (release builds abort right after).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(file) = FILE.get() {
            let message = info.payload_as_str().unwrap_or("(no message)");
            let place = info.location().map_or_else(|| "?".into(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            let line = format!("{} [panic] {}: {message} at {place}\n", utc_now(), thread_name());
            // The lock may be poisoned by an earlier panic; the file is still fine.
            let _ = file.lock().unwrap_or_else(PoisonError::into_inner).write_all(line.as_bytes());
        }
        previous(info);
    }));
}

/// Open the log for appending, moving a too-big one aside first.
fn open(path: &std::path::Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        std::fs::rename(path, path.with_extension("log.1"))?;
    }
    File::options().create(true).append(true).open(path)
}

#[doc(hidden)]
pub fn write(level: Level, args: std::fmt::Arguments) {
    // Seconds since the first log line: enough to line up events across threads.
    static START: OnceLock<Instant> = OnceLock::new();
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let name = format!("{level:?}").to_lowercase();
    if level <= max_level() {
        eprintln!("haze {t:9.3} [{name}]: {args}");
    }
    if level <= FILE_LEVEL
        && let Some(file) = FILE.get()
    {
        let line = format!("{} {t:9.3} [{name}] {}: {args}\n", utc_now(), thread_name());
        let _ = file.lock().unwrap().write_all(line.as_bytes());
    }
}

fn thread_name() -> String {
    std::thread::current().name().unwrap_or("?").to_string()
}

/// Wall-clock time as `YYYY-MM-DD HH:MM:SS.mmmZ` (UTC), so the file can be matched
/// with when something was heard.
fn utc_now() -> String {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    utc(since.as_secs(), since.subsec_millis())
}

fn utc(secs: u64, millis: u32) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[macro_export]
macro_rules! log_at {
    ($level:expr, $($arg:tt)*) => {
        if $crate::log::enabled($level) {
            $crate::log::write($level, format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! log_error { ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Error, $($arg)*) }; }
#[macro_export]
macro_rules! log_warn { ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Warn, $($arg)*) }; }
#[macro_export]
macro_rules! log_info { ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Info, $($arg)*) }; }
#[macro_export]
macro_rules! log_debug { ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Debug, $($arg)*) }; }
#[macro_export]
macro_rules! log_trace { ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Trace, $($arg)*) }; }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse(" DEBUG "), Some(Level::Debug));
        assert_eq!(Level::parse("nope"), None);
        assert!(Level::Error < Level::Warn && Level::Debug < Level::Trace);
    }

    #[test]
    fn log_file_appends_and_rotates() {
        let dir = std::env::temp_dir().join(format!("haze-log-test-{}", std::process::id()));
        let path = dir.join("logs").join("haze.log");
        writeln!(open(&path).unwrap(), "one").unwrap();
        writeln!(open(&path).unwrap(), "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        // An append-only handle may not change the length (Windows), so open it for writing.
        File::options().write(true).open(&path).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
        writeln!(open(&path).unwrap(), "fresh").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
        assert_eq!(std::fs::metadata(path.with_extension("log.1")).unwrap().len(), MAX_FILE_BYTES + 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0, 0), "1970-01-01 00:00:00.000Z");
        assert_eq!(utc(951_782_400, 5), "2000-02-29 00:00:00.005Z");
        assert_eq!(utc(1_791_983_045, 120), "2026-10-14 13:04:05.120Z");
    }
}
