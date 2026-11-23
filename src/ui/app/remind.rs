//! Reminders from the guide: a programme still to come is picked, and a minute
//! before it starts a notice offers to watch it (bringing the window forward if
//! it's behind others).

use super::{App, Playing};
use crate::iptv::model::Programme;
use crate::iptv::time;
use crate::library::Reminder;
use eframe::egui::{self, UserAttentionType, ViewportCommand};
use std::time::Duration;

/// How often to look while the next reminder is far off: the UI may otherwise sleep.
const LOOK: i64 = 60;

impl App {
    /// Be reminded of `programme` on channel `key`, or not any more.
    pub(super) fn toggle_reminder(&mut self, key: &str, programme: &Programme) {
        if programme.start <= time::now() {
            return;
        }
        let source = self.source_id();
        if self.reminders.toggle(&source, key, programme) {
            let at = crate::ui::views::hhmm(programme.start - crate::library::REMIND_LEAD);
            self.show_toast(&format!("Reminder set: {} (at {at})", programme.title));
        } else {
            self.show_toast("Reminder removed");
        }
    }

    /// Show the reminders whose time has come, and drop those that are over.
    pub(super) fn remind_when_due(&mut self, ctx: &egui::Context) {
        let now = time::now();
        self.reminding.retain(|r| r.stop > now);
        let due = self.reminders.due(now);
        if !due.is_empty() {
            let source = self.source_id();
            let playing = match &self.session.playing {
                Some(Playing::Live { key }) => Some(key.as_str()),
                _ => None,
            };
            // What's on already needs no reminder.
            let new: Vec<Reminder> = due.into_iter().filter(|r| !(r.source == source && playing == Some(r.channel.as_str()))).collect();
            if !new.is_empty() {
                log_info!("reminder: {} programme(s) starting", new.len());
                if !ctx.input(|i| i.focused) {
                    ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(UserAttentionType::Informational));
                }
                self.reminding.extend(new);
            }
        }
        if let Some(at) = self.reminders.next_at() {
            ctx.request_repaint_after(Duration::from_secs((at - now).clamp(1, LOOK) as u64));
        }
        if let Some(stop) = self.reminding.iter().map(|r| r.stop).min() {
            ctx.request_repaint_after(Duration::from_secs((stop - now).clamp(1, LOOK) as u64));
        }
    }

    /// Answer the reminder shown first: watch its programme, or let it go.
    pub(super) fn answer_reminder(&mut self, ctx: &egui::Context, watch: bool) {
        if self.reminding.is_empty() {
            return;
        }
        let r = self.reminding.remove(0);
        if !watch {
            return;
        }
        if self.mini_on() {
            self.toggle_mini(ctx);
        }
        if r.source == self.source_id() {
            if self.channel(&r.channel).is_some() {
                self.play(ctx, Playing::Live { key: r.channel });
            } else {
                self.show_toast("That channel isn't in the list any more");
            }
        } else if let Some(i) = self.sources.iter().position(|s| s.id == r.source) {
            // Another source's channel: switch to it, and play it once it's loaded.
            self.pick_source(ctx, i);
            self.resume = Some(r.channel);
        }
    }

    /// The name of the reminder's channel, when it's of the source in use.
    pub fn reminder_channel(&self, r: &Reminder) -> Option<&str> {
        (r.source == self.source_id()).then(|| self.channel(&r.channel)).flatten().map(|c| c.name.as_str())
    }
}
