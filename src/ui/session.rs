//! What plays: the player, what it was opened for, and opening a live channel
//! again when the provider drops it.

use crate::iptv::model::{Episode, Programme};
use crate::media::{Error, Player, Status};
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

/// How many times a failure is worth trying again: once when the provider said no
/// (a connection that's only now closing at its end), never when the stream can't
/// be decoded, a few times otherwise (the network, a server that hiccups).
fn attempts(status: &Status) -> u32 {
    match status {
        Status::Failed(Error::Refused | Error::NotFound) => 1,
        Status::Failed(Error::Decode(_) | Error::Start(_)) => 0,
        _ => RETRIES,
    }
}

#[derive(Default)]
pub struct Session {
    pub player: Option<Player>,
    pub playing: Option<Playing>,
    /// Stream time positions count from (where the stream starts), so they start at 0.
    pub start_pts: Option<f64>,
    /// The picture's shape, when the viewer set one for this stream.
    pub shape: crate::ui::video::Shape,
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

    /// The key of the movie or episode playing, whose progress is kept.
    pub fn recording_key(&self) -> Option<String> {
        match self.playing.as_ref()? {
            Playing::Movie { key } => Some(key.clone()),
            Playing::Episode { episode, .. } => Some(episode.key.clone()),
            Playing::Live { .. } | Playing::Catchup { .. } => None,
        }
    }

    pub fn paused(&self) -> bool {
        self.player.as_ref().is_some_and(Player::paused)
    }

    /// What `status` calls for at `now`. A live channel that fails or ends is opened
    /// again, waiting a little longer each time, until it has been tried a few times
    /// without playing: a provider isn't asked for a dead stream over and over.
    pub fn next(&mut self, status: &Status, now: Instant) -> Next {
        match status {
            Status::Failed(_) | Status::Ended if self.live() && self.retries < attempts(status) => {
                let at = *self.retry_at.get_or_insert_with(|| now + Duration::from_secs(1 + u64::from(self.retries)));
                if now < at {
                    return Next::Wait(at);
                }
                match status {
                    Status::Failed(e) => log_info!("reconnecting ({e})"),
                    _ => log_info!("reconnecting (the stream ended)"),
                }
                self.retries += 1;
                Next::Reopen
            }
            Status::Ended if !self.live() => Next::Ended,
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
        let failed = Status::Failed(Error::Stream("gone".into()));
        assert_eq!(s.next(&failed, t), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(s.next(&failed, t + Duration::from_millis(500)), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(s.next(&failed, t + Duration::from_secs(1)), Next::Reopen);
        s.retry_at = None; // as `restart` leaves it
        assert_eq!(s.next(&failed, t), Next::Wait(t + Duration::from_secs(2)));
    }

    #[test]
    fn gives_up_after_a_few_attempts_until_it_plays_again() {
        let mut s = live();
        let failed = Status::Failed(Error::Stream("gone".into()));
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
    fn a_refusal_is_tried_once_and_a_broken_stream_not_at_all() {
        let (mut s, t) = (live(), Instant::now());
        let refused = Status::Failed(Error::Refused);
        assert_eq!(s.next(&refused, t), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(s.next(&refused, t + Duration::from_secs(1)), Next::Reopen);
        s.retry_at = None;
        assert_eq!(s.next(&refused, t + Duration::from_secs(9)), Next::Nothing);
        assert_eq!(live().next(&Status::Failed(Error::Decode("the picture".into())), t), Next::Nothing);
    }

    #[test]
    fn a_channel_that_keeps_ending_is_tried_a_few_times() {
        let mut s = live();
        let t = Instant::now();
        for n in 0..RETRIES {
            let at = t + Duration::from_secs(1 + u64::from(n));
            assert_eq!(s.next(&Status::Ended, t), Next::Wait(at));
            assert_eq!(s.next(&Status::Ended, at), Next::Reopen);
            s.retry_at = None;
        }
        assert_eq!(s.next(&Status::Ended, t), Next::Nothing);
    }

    #[test]
    fn a_recording_ends_and_a_channel_goes_on() {
        let (mut channel, t) = (live(), Instant::now());
        assert_eq!(channel.next(&Status::Ended, t), Next::Wait(t + Duration::from_secs(1)));
        assert_eq!(channel.next(&Status::Ended, t + Duration::from_secs(1)), Next::Reopen);
        let mut movie = watching(Playing::Movie { key: "5".into() });
        assert_eq!(movie.next(&Status::Ended, Instant::now()), Next::Ended);
        assert_eq!(movie.next(&Status::Failed(Error::Stream("gone".into())), Instant::now()), Next::Nothing);
    }
}
