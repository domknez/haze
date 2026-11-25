//! Recording the live channel that plays: a folder is picked (the one used last is
//! offered first), and the channel goes into a file there until the recording is
//! stopped, another channel is picked, or the app closes. When the provider drops
//! the stream, the recording carries on in a new file once it's back.

use super::{App, Playing};
use crate::iptv::time;
use crate::media::Recording;
use crate::ui::load::Load;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A recording under way.
pub struct Record {
    /// The channel recorded (its key).
    key: String,
    /// Its name, which the files are named after.
    channel: String,
    folder: PathBuf,
    pub since: Instant,
    /// The files written so far, one more each time the stream came back.
    files: Vec<PathBuf>,
}

impl App {
    /// Start recording, asking for the folder first, or stop.
    pub(super) fn toggle_recording(&mut self, ctx: &egui::Context) {
        if self.record.is_some() {
            self.stop_recording();
        } else if !self.can_record() {
            self.show_toast("Only a live channel can be recorded");
        } else if !self.folder_pick.is_loading() {
            let start = self.prefs.record_folder.as_deref().map(PathBuf::from).filter(|f| f.is_dir()).or_else(default_folder);
            self.folder_pick = Load::start(ctx, move || Ok(pick_folder(start)));
        }
    }

    /// Whether what plays can be recorded: a live channel, watched alone.
    pub fn can_record(&self) -> bool {
        self.session.live() && self.session.player.is_some() && !self.multi.on()
    }

    /// The folder picked, and whether the recording still goes on: the same channel
    /// plays, and its player records (one that took over after a drop-out is told to).
    pub(super) fn poll_recording(&mut self) {
        if self.folder_pick.poll() {
            if let Load::Ready(Some(folder)) = std::mem::take(&mut self.folder_pick) {
                self.start_recording(folder);
            }
            self.folder_pick = Load::Idle;
        }
        let Some(record) = &self.record else { return };
        let same = !self.multi.on() && matches!(&self.session.playing, Some(Playing::Live { key }) if *key == record.key);
        let state = self.session.player.as_ref().map(|p| p.recording());
        match state {
            _ if !same => self.stop_recording(),
            Some(Recording::Off) => self.record_next_file(),
            Some(Recording::Failed(why)) => {
                self.record = None;
                self.show_toast(&format!("Recording stopped: {why}"));
            }
            Some(Recording::To(_)) | None => {}
        }
    }

    fn start_recording(&mut self, folder: PathBuf) {
        let Some(Playing::Live { key }) = self.session.playing.clone() else { return };
        if !self.can_record() {
            return;
        }
        let channel = self.channel(&key).map_or_else(|| "Recording".to_string(), |c| c.name.clone());
        self.prefs.update(|p| p.record_folder = Some(folder.to_string_lossy().into_owned()));
        log_info!("recording a channel");
        self.show_toast(&format!("Recording {channel} (R stops it)"));
        self.record = Some(Record { key, channel, folder, since: Instant::now(), files: Vec::new() });
        self.record_next_file();
    }

    /// Have the player record into a new file.
    fn record_next_file(&mut self) {
        let (Some(record), Some(player)) = (&mut self.record, &self.session.player) else { return };
        let path = new_file(&record.folder, &record.channel, time::now());
        player.record(Some(path.clone()));
        record.files.push(path);
    }

    pub(super) fn stop_recording(&mut self) {
        let Some(record) = self.record.take() else { return };
        if let Some(player) = &self.session.player {
            player.record(None);
        }
        let saved: Vec<&PathBuf> = record.files.iter().filter(|f| f.exists()).collect();
        match saved[..] {
            [] => self.show_toast("Recording stopped before anything was saved"),
            [file] => self.show_toast(&format!("Recording saved: {}", file_name(file))),
            _ => self.show_toast(&format!("Recording saved: {} files in {}", saved.len(), file_name(&record.folder))),
        }
    }
}

/// Ask for the folder to record into, starting at `start`; None when cancelled.
fn pick_folder(start: Option<PathBuf>) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title("Record to");
    match start {
        Some(start) => dialog.set_directory(start),
        None => dialog,
    }
    .pick_folder()
}

/// The system's folder for videos: `~/Movies` on macOS, `Videos` elsewhere.
fn default_folder() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    let videos = home.join(if cfg!(target_os = "macos") { "Movies" } else { "Videos" });
    Some(if videos.is_dir() { videos } else { home })
}

/// A file in `folder` not there yet, named after the channel and the local time:
/// `HRT 1 2026-11-25 20.15.mkv`.
fn new_file(folder: &Path, channel: &str, now: i64) -> PathBuf {
    let c = time::civil(now + crate::ui::views::local_offset(now));
    let stem = format!("{} {}-{:02}-{:02} {:02}.{:02}", safe_name(channel), c.year, c.month, c.day, c.hour, c.minute);
    (1..)
        .map(|n| folder.join(if n == 1 { format!("{stem}.mkv") } else { format!("{stem} ({n}).mkv") }))
        .find(|path| !path.exists())
        .unwrap_or_else(|| folder.join(format!("{stem}.mkv")))
}

/// `name` fit to be part of a file's name on any system.
fn safe_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { ' ' } else { c }).collect();
    let safe = safe.split_whitespace().collect::<Vec<_>>().join(" ");
    let safe = safe.trim_matches('.');
    if safe.is_empty() { "Recording".into() } else { safe.into() }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_files_safely() {
        assert_eq!(safe_name("HRT 1"), "HRT 1");
        assert_eq!(safe_name("News: 24/7 <HD>"), "News 24 7 HD");
        assert_eq!(safe_name("..."), "Recording");
        assert_eq!(safe_name(" a\tb\n"), "a b");
    }

    #[test]
    fn never_writes_over_a_file() {
        let folder = std::env::temp_dir().join(format!("haze-record-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let first = new_file(&folder, "HRT 1", 0);
        assert!(file_name(&first).starts_with("HRT 1 "), "{first:?}");
        assert!(file_name(&first).ends_with(".mkv"));
        std::fs::write(&first, b"").unwrap();
        let second = new_file(&folder, "HRT 1", 0);
        assert_ne!(first, second);
        assert!(file_name(&second).ends_with(" (2).mkv"), "{second:?}");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
