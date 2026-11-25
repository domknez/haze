//! Recording programmes from the guide: one on now or still to come is recorded in
//! the background, on a connection of its own, whatever plays meanwhile. It starts
//! a minute before the programme and goes on for two after it ends (the guide's times
//! are seldom exact), into the folder recordings go to. Haze has to be running.

use super::record::{file_name, new_file, saved_text};
use super::{App, Playing};
use crate::iptv::model::Programme;
use crate::iptv::time;
use crate::library::Plan;
use crate::media::Taping;
use eframe::egui;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a recording goes on after its programme ends, in seconds.
const OVERRUN: i64 = 120;

/// How often to look while the next recording is far off: the UI may otherwise sleep.
const LOOK: i64 = 60;

/// A programme being recorded.
pub struct Taped {
    pub plan: Plan,
    taping: Taping,
    folder: PathBuf,
    /// When it ends (Unix seconds).
    until: i64,
    pub since: Instant,
}

impl App {
    /// Record `programme` on channel `key`, or not any more (stopping it, if it's
    /// being recorded).
    pub(super) fn toggle_scheduled(&mut self, key: &str, programme: &Programme) {
        let source = self.source_id();
        if let Some(i) = self.taped.iter().position(|t| t.plan.is(&source, key, programme.start)) {
            let taped = self.taped.remove(i);
            return self.taping_over(taped, true);
        }
        if programme.stop <= time::now() {
            return;
        }
        if self.recordings.toggle(&source, key, programme) {
            let (from, to) = (crate::ui::views::hhmm(programme.start), crate::ui::views::hhmm(programme.stop));
            let into = self.record_folder().map(|f| format!(" into {}", file_name(&f))).unwrap_or_default();
            self.show_toast(&format!("Will record {} ({from}–{to}){into}", programme.title));
        } else {
            self.show_toast("Recording cancelled");
        }
    }

    /// Start the recordings whose time came, and see to those that are over.
    pub(super) fn record_when_due(&mut self, ctx: &egui::Context) {
        let now = time::now();
        // The channels are needed to start one: just after haze starts, they're loading.
        let ready = self.catalog.ready().is_some();
        let due: Vec<Plan> = self.recordings.due(now).filter(|p| ready && !self.taped.iter().any(|t| t.plan == **p)).cloned().collect();
        for plan in due {
            self.start_taping(ctx, plan, now);
        }
        let (over, going): (Vec<Taped>, Vec<Taped>) = std::mem::take(&mut self.taped).into_iter().partition(|t| t.taping.tape().over);
        self.taped = going;
        for taped in over {
            self.taping_over(taped, false);
        }
        if self.taped.is_empty() {
            self.awake_recording = None;
        } else if self.awake_recording.is_none() {
            self.awake_recording = Some(crate::awake::Awake::recording());
        }
        if let Some(at) = self.recordings.next_at(now) {
            ctx.request_repaint_after(Duration::from_secs((at - now).clamp(1, LOOK) as u64));
        }
    }

    fn start_taping(&mut self, ctx: &egui::Context, plan: Plan, now: i64) {
        let (channel, folder, request) = match self.ready_to_tape(&plan) {
            Ok(ready) => ready,
            Err(why) => return self.not_taped(&plan, why),
        };
        let watching = usize::from(self.session.player.is_some()) + self.multi.tiles.len();
        match starting(self.streams_allowed(), watching, self.taped.len()) {
            Start::Go => self.show_toast(&format!("Recording {} on {channel}", plan.title)),
            Start::StopWatching => {
                self.stop();
                self.show_toast(&format!("Stopped watching to record {}: your provider allows no more streams at once", plan.title));
            }
            Start::Cant(why) => return self.not_taped(&plan, why),
        }
        log_info!("recording a programme");
        let name = format!("{channel} - {}", plan.title);
        let until = plan.stop + OVERRUN;
        let ends = Instant::now() + Duration::from_secs((until - now).max(0) as u64);
        let (into, repaint) = (folder.clone(), ctx.clone());
        let taping = Taping::start(request, ends, move || new_file(&into, &name, time::now()), move || repaint.request_repaint());
        self.taped.push(Taped { plan, taping, folder, until, since: Instant::now() });
    }

    /// The channel's name, the folder and the stream to record `plan` from, or why
    /// it can't be.
    fn ready_to_tape(&self, plan: &Plan) -> Result<(String, PathBuf, crate::media::Request), &'static str> {
        if plan.source != self.source_id() {
            return Err("its source isn't the one open");
        }
        let channel = self.channel(&plan.channel).ok_or("the channel isn't in the list any more")?;
        let folder = self.record_folder().ok_or("there's no folder to record into")?;
        let request = self.request_for(&Playing::Live { key: plan.channel.clone() }).ok_or("the channel can't be played")?;
        Ok((channel.name.clone(), folder, request))
    }

    fn not_taped(&mut self, plan: &Plan, why: &str) {
        log_warn!("recording: couldn't start ({why})");
        self.recordings.remove(&plan.source, &plan.channel, plan.start);
        self.show_toast(&format!("Couldn't record {}: {why}", plan.title));
    }

    /// Finish a recording, `stopped` before its time or over, and say what it saved.
    fn taping_over(&mut self, taped: Taped, stopped: bool) {
        let Taped { plan, taping, folder, until, .. } = taped;
        let tape = taping.finish();
        self.recordings.remove(&plan.source, &plan.channel, plan.start);
        let saved: Vec<&PathBuf> = tape.files.iter().filter(|f| f.exists()).collect();
        // One that ended on its own before its time couldn't go on.
        let failed = tape.problem.as_deref().filter(|_| !stopped && time::now() < until);
        self.show_toast(&taped_text(&plan.title, &saved, &folder, failed));
    }

    /// How many streams the provider allows at once, when it says.
    fn streams_allowed(&self) -> Option<u32> {
        self.catalog.ready().and_then(|c| c.account.as_ref()?.max_connections).filter(|&m| m > 0)
    }

    /// How many more streams may be watched besides those recording, when the
    /// provider says how many it allows.
    pub(super) fn streams_to_watch(&self) -> Option<usize> {
        self.streams_allowed().map(|m| (m as usize).saturating_sub(self.taped.len()))
    }
}

/// What starting a recording calls for.
#[derive(Debug, PartialEq)]
enum Start {
    /// There's a stream to spare.
    Go,
    /// There will be once watching stops.
    StopWatching,
    /// It can't be recorded, for this reason.
    Cant(&'static str),
}

/// What starting a recording calls for, with the provider allowing `allowed`
/// streams at once (when it says), `watching` of them watched and `recording`
/// recorded.
fn starting(allowed: Option<u32>, watching: usize, recording: usize) -> Start {
    match allowed.map(|m| m as usize) {
        Some(m) if recording >= m => Start::Cant("every stream your provider allows is recording"),
        Some(m) if watching + recording >= m => Start::StopWatching,
        _ => Start::Go,
    }
}

/// What a recording of `title` saved into `folder`, and why it stopped early, if
/// it `failed`.
fn taped_text(title: &str, files: &[&PathBuf], folder: &std::path::Path, failed: Option<&str>) -> String {
    match (files, failed) {
        ([], Some(why)) => format!("Couldn't record {title}: {why}"),
        ([], None) => format!("Nothing of {title} was recorded"),
        (_, Some(why)) => format!("Recording of {title} stopped: {why}. {}", saved_text(files, folder)),
        (_, None) => saved_text(files, folder),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_beside_what_plays_when_the_provider_allows() {
        assert_eq!(starting(None, 4, 3), Start::Go, "no limit known");
        assert_eq!(starting(Some(2), 1, 0), Start::Go);
        assert_eq!(starting(Some(3), 1, 1), Start::Go);
    }

    #[test]
    fn stops_watching_to_record_when_it_must() {
        assert_eq!(starting(Some(1), 1, 0), Start::StopWatching);
        assert_eq!(starting(Some(2), 2, 0), Start::StopWatching, "multiview");
        assert_eq!(starting(Some(2), 1, 1), Start::StopWatching);
    }

    #[test]
    fn cant_record_more_than_the_provider_allows() {
        assert!(matches!(starting(Some(1), 0, 1), Start::Cant(_)));
        assert!(matches!(starting(Some(2), 0, 2), Start::Cant(_)));
    }

    #[test]
    fn says_what_was_recorded() {
        let folder = PathBuf::from("/Movies");
        let file = folder.join("HRT 1 - News 2026-11-25 19.59.mkv");
        assert_eq!(taped_text("News", &[&file], &folder, None), "Recording saved: HRT 1 - News 2026-11-25 19.59.mkv");
        assert_eq!(taped_text("News", &[], &folder, None), "Nothing of News was recorded");
        assert_eq!(
            taped_text("News", &[], &folder, Some("couldn't create the file: Permission denied")),
            "Couldn't record News: couldn't create the file: Permission denied"
        );
        assert_eq!(
            taped_text("News", &[&file, &file], &folder, Some("couldn't write to the file: No space left on device")),
            "Recording of News stopped: couldn't write to the file: No space left on device. Recording saved: 2 files in Movies"
        );
    }
}
