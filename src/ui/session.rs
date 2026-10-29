//! What plays: the player, what it was opened for, and opening a live channel
//! again when the provider drops it.

use crate::iptv::model::{Episode, Programme};
use crate::media::{Player, Status};
use std::time::{Duration, Instant};

/// What's playing.
#[derive(Clone, Debug, PartialEq)]
pub enum Playing {
    Live {
        key: String,
    },
    /// A programme from a channel's archive.
    Catchup {
        key: String,
        programme: Programme,
    },
    Movie {
        key: String,
    },
    /// An episode of the series with key `series`.
    Episode {
        series: String,
        episode: Episode,
    },
}

impl Playing {
    pub fn key(&self) -> &str {
        match self {
            Playing::Live { key } | Playing::Catchup { key, .. } | Playing::Movie { key } => key,
            Playing::Episode { episode, .. } => &episode.key,
        }
    }
}

/// Times a live channel that failed is opened again before giving up.
const RETRIES: u32 = 5;

#[derive(Default)]
pub struct Session {
    pub player: Option<Player>,
    pub playing: Option<Playing>,
    /// Stream time of the first frame of a recording, so positions start at 0.
    pub start_pts: Option<f64>,
    /// Times the live channel was opened again since it last played.
    retries: u32,
    retry_at: Option<Instant>,
}

/// What to do about the player's status.
#[derive(Debug, PartialEq)]
pub enum Next {
    Nothing,
    /// Look again then: the channel is opened again after a pause.
    Wait(Instant),
    /// Open what plays again.
    Reopen,
    /// It played to the end.
    Ended,
}

impl Session {
    pub fn start(&mut self, playing: Playing, player: Player) {
        *self = Session { player: Some(player), playing: Some(playing), ..Session::default() };
    }

    /// The same again, after it dropped; the attempts are still counted.
    pub fn restart(&mut self, player: Player) {
        self.player = Some(player);
        self.start_pts = None;
        self.retry_at = None;
    }

    pub fn stop(&mut self) {
        *self = Session::default();
    }

    pub fn live(&self) -> bool {
        matches!(self.playing, Some(Playing::Live { .. }))
    }

    pub fn paused(&self) -> bool {
        self.player.as_ref().is_some_and(Player::paused)
    }

    /// What `status` calls for at `now`. A live channel that fails is opened again,
    /// waiting a little longer each time; one that ends is opened again at once.
    pub fn next(&mut self, status: &Status, now: Instant) -> Next {
        match status {
            Status::Failed(e) if self.live() && self.retries < RETRIES => {
                let at = *self.retry_at.get_or_insert_with(|| now + Duration::from_secs(1 + u64::from(self.retries)));
                if now < at {
                    return Next::Wait(at);
                }
                log_info!("reconnecting ({e})");
                self.retries += 1;
                Next::Reopen
            }
            Status::Ended if self.live() => {
                self.retries += 1;
                Next::Reopen
            }
            Status::Ended => Next::Ended,
            Status::Playing => {
                self.retries = 0;
                Next::Nothing
            }
            _ => Next::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watching(playing: Playing) -> Session {
        Session { playing: Some(playing), ..Session::default() }
    }

    fn live() -> Session {
        watching(Playing::Live { key: "1".into() })
    }

    #[test]
    fn opens_a_failed_channel_again_waiting_longer_each_time() {
        let mut s = live();
        let t = Instant::now();
        let failed = Status::Failed("gone".into());
        assert_eq!(s.next(&failed, t), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(s.next(&failed, t + Duration::from_millis(500)), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(s.next(&failed, t + Duration::from_secs(1)), Next::Reopen);
        s.retry_at = None; // as `restart` leaves it
        assert_eq!(s.next(&failed, t), Next::Wait(t + Duration::from_secs(2)));
    }

    #[test]
    fn gives_up_after_a_few_attempts_until_it_plays_again() {
        let mut s = live();
        let failed = Status::Failed("gone".into());
        let t = Instant::now();
        let fail = |s: &mut Session| {
            let Next::Wait(at) = s.next(&failed, t) else { return s.next(&failed, t) };
            let next = s.next(&failed, at);
            s.retry_at = None;
            next
        };
        for _ in 0..RETRIES {
            assert_eq!(fail(&mut s), Next::Reopen);
        }
        assert_eq!(fail(&mut s), Next::Nothing);
        s.next(&Status::Playing, t);
        assert_eq!(fail(&mut s), Next::Reopen);
    }

    #[test]
    fn a_recording_ends_and_a_channel_goes_on() {
        assert_eq!(live().next(&Status::Ended, Instant::now()), Next::Reopen);
        let mut movie = watching(Playing::Movie { key: "5".into() });
        assert_eq!(movie.next(&Status::Ended, Instant::now()), Next::Ended);
        assert_eq!(movie.next(&Status::Failed("gone".into()), Instant::now()), Next::Nothing);
    }
}
