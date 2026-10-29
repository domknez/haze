//! What a source's playlist turns into: channels in groups, and the guide.

use std::collections::HashMap;

/// One live channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// Stable within its source (the provider's stream id, or the stream URL), so
    /// favourites and the last channel survive a playlist refresh.
    pub key: String,
    pub name: String,
    /// The channel number the provider gives it, if any.
    pub number: Option<u32>,
    pub logo: Option<String>,
    pub group: String,
    pub url: String,
    /// The channel's id in the guide (`tvg-id`, Xtream's `epg_channel_id`).
    pub epg_id: Option<String>,
    pub catchup: Option<Catchup>,
    /// Extra HTTP headers some providers require (`#EXTVLCOPT:http-user-agent=…`).
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

/// How to play a programme that already aired.
#[derive(Clone, Debug, PartialEq)]
pub struct Catchup {
    /// How far back the archive goes.
    pub days: u32,
    pub kind: CatchupKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatchupKind {
    /// Xtream Codes' `timeshift` endpoint.
    Xtream { stream_id: u64 },
    /// `catchup-source` template with `{utc}`, `{start}`, `{duration}`, … placeholders,
    /// appended to the stream URL (`append`) or used as the whole URL (`default`).
    Template { source: String, append: bool },
    /// `?utc=<start>&lutc=<now>` appended to the stream URL (`shift`, `timeshift`).
    Shift,
    /// Flussonic: `…/index-<start>-<duration>.m3u8`.
    Flussonic,
}

/// One programme in the guide; times are Unix seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Programme {
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub desc: String,
}

impl Programme {
    pub fn is_on(&self, now: i64) -> bool {
        self.start <= now && now < self.stop
    }

    /// 0.0 ..= 1.0 through the programme at `now`.
    pub fn progress(&self, now: i64) -> f32 {
        let length = (self.stop - self.start).max(1) as f32;
        ((now - self.start) as f32 / length).clamp(0.0, 1.0)
    }
}

/// A movie from a provider's video library.
#[derive(Clone, Debug, PartialEq)]
pub struct Movie {
    pub key: String,
    pub name: String,
    pub poster: Option<String>,
    pub group: String,
    pub url: String,
    pub rating: Option<f32>,
    pub year: Option<u32>,
}

/// A series from a provider's video library.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// The provider's series id (Xtream), or the group and name (playlists).
    pub key: String,
    pub name: String,
    pub poster: Option<String>,
    pub group: String,
    pub rating: Option<f32>,
    pub year: Option<u32>,
    /// What it's about (Xtream).
    pub plot: Option<String>,
    /// In season order. Playlists list every episode; Xtream providers give them
    /// when the series is opened (see `xtream::Login::episodes`).
    pub episodes: Vec<Episode>,
}

/// One episode of a series.
#[derive(Clone, Debug, PartialEq)]
pub struct Episode {
    pub key: String,
    pub season: u32,
    pub number: u32,
    pub title: String,
    pub url: String,
}

impl Episode {
    /// `S1 E4`
    pub fn code(&self) -> String {
        format!("S{} E{}", self.season, self.number)
    }
}

/// Where a playlist group's entries go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelf {
    Channels,
    Movies,
    Series,
}

impl Shelf {
    pub const ALL: [Shelf; 3] = [Shelf::Channels, Shelf::Movies, Shelf::Series];

    pub fn key(self) -> &'static str {
        match self {
            Shelf::Channels => "channels",
            Shelf::Movies => "movies",
            Shelf::Series => "series",
        }
    }

    pub fn from_key(k: &str) -> Option<Shelf> {
        Shelf::ALL.into_iter().find(|s| s.key() == k)
    }
}

/// Everything a source offers.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub channels: Vec<Channel>,
    /// Channel groups in playlist order.
    pub groups: Vec<String>,
    pub movies: Vec<Movie>,
    pub movie_groups: Vec<String>,
    pub series: Vec<Series>,
    pub series_groups: Vec<String>,
    /// The guide the playlist points to (`url-tvg` / `x-tvg-url`), if any.
    pub epg_url: Option<String>,
    /// What the provider says about the account (Xtream only).
    pub account: Option<Account>,
}

impl Catalog {
    /// Groups in order of first appearance among `channels`.
    pub fn groups_of(channels: &[Channel]) -> Vec<String> {
        first_seen(channels.iter().map(|c| c.group.as_str()))
    }

    pub fn groups_of_movies(movies: &[Movie]) -> Vec<String> {
        first_seen(movies.iter().map(|m| m.group.as_str()))
    }

    pub fn groups_of_series(series: &[Series]) -> Vec<String> {
        first_seen(series.iter().map(|s| s.group.as_str()))
    }
}

fn first_seen<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names.filter(|n| seen.insert(*n)).map(str::to_string).collect()
}

/// An Xtream Codes account's status.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub status: String,
    /// When the subscription ends (Unix seconds); none for unlimited.
    pub expires: Option<i64>,
    pub max_connections: Option<u32>,
    pub active_connections: Option<u32>,
    /// The provider's clock, in seconds east of UTC (archive URLs use it).
    pub server_offset: i64,
}

/// Guide data by guide channel id, each channel's programmes sorted by start.
#[derive(Clone, Debug, Default)]
pub struct Guide {
    pub programmes: HashMap<String, Vec<Programme>>,
}

impl Guide {
    pub fn for_channel(&self, channel: &Channel) -> &[Programme] {
        channel.epg_id.as_ref().and_then(|id| self.programmes.get(id)).map_or(&[], Vec::as_slice)
    }

    /// The programme on at `now` and the index of it in the channel's list.
    pub fn now_on(&self, channel: &Channel, now: i64) -> Option<(usize, &Programme)> {
        let list = self.for_channel(channel);
        let i = list.partition_point(|p| p.stop <= now);
        list.get(i).filter(|p| p.is_on(now)).map(|p| (i, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programme(start: i64, stop: i64) -> Programme {
        Programme { start, stop, title: format!("{start}"), desc: String::new() }
    }

    #[test]
    fn finds_the_programme_on_now() {
        let channel = Channel {
            key: "1".into(),
            name: "One".into(),
            number: None,
            logo: None,
            group: String::new(),
            url: String::new(),
            epg_id: Some("one".into()),
            catchup: None,
            user_agent: None,
            referrer: None,
        };
        let mut guide = Guide::default();
        guide.programmes.insert("one".into(), vec![programme(0, 10), programme(10, 20), programme(30, 40)]);
        assert_eq!(guide.now_on(&channel, 15).map(|(i, _)| i), Some(1));
        assert_eq!(guide.now_on(&channel, 10).map(|(i, _)| i), Some(1));
        assert!(guide.now_on(&channel, 25).is_none(), "a gap in the guide");
        assert!(guide.now_on(&channel, 45).is_none());
        assert_eq!(programme(10, 20).progress(15), 0.5);
    }
}
