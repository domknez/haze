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

/// A name without the notes public lists put after it: `HRT 1 (720p) [Geo-blocked]`
/// and `HRT 1 Ⓖ` are `HRT 1`.
fn without_labels(mut name: &str) -> &str {
    loop {
        let before = name;
        for label in ["[Geo-blocked]", "[Not 24/7]", "Ⓖ", "Ⓢ", "Ⓨ"] {
            name = name.strip_suffix(label).unwrap_or(name).trim_end();
        }
        if let Some(head) = name.strip_suffix("p)")
            && let Some((head, digits)) = head.rsplit_once('(')
            && (3..=4).contains(&digits.len())
            && digits.bytes().all(|b| b.is_ascii_digit())
            && !head.trim_end().is_empty()
        {
            name = head.trim_end();
        }
        if name == before {
            return name;
        }
    }
}

/// A name as it should read: accents joined to their letters, and the small raised
/// letters providers tag channels with (`ᶠʰᵈ`, `ᴴᴰ`, `ʰᵉᵛᶜ`), which most fonts lack,
/// written as plain capitals (`FHD`).
pub fn tidy_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let name = without_labels(name.trim());
    if name.is_ascii() {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let mut chars = name.nfc().peekable();
    while let Some(c) = chars.next() {
        if raised(c).is_none() {
            out.push(c);
            continue;
        }
        let mut run = vec![c];
        while let Some(&n) = chars.peek().filter(|n| raised(**n).is_some()) {
            run.push(n);
            chars.next();
        }
        // Raised digits on their own (`m²`) stay as they are.
        if !run.iter().any(|c| raised(*c).is_some_and(|r| r.is_ascii_alphabetic())) {
            out.extend(run);
            continue;
        }
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.extend(run.into_iter().filter_map(raised));
        if chars.peek().is_some_and(|n| n.is_alphanumeric()) {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// The plain character a raised (modifier or superscript) letter, digit or plus
/// stands for.
fn raised(c: char) -> Option<char> {
    Some(match c {
        'ᵃ' | 'ᴬ' => 'A',
        'ᵇ' | 'ᴮ' => 'B',
        'ᶜ' => 'C',
        'ᵈ' | 'ᴰ' => 'D',
        'ᵉ' | 'ᴱ' => 'E',
        'ᶠ' => 'F',
        'ᵍ' | 'ᴳ' => 'G',
        'ʰ' | 'ᴴ' => 'H',
        'ⁱ' | 'ᴵ' => 'I',
        'ʲ' | 'ᴶ' => 'J',
        'ᵏ' | 'ᴷ' => 'K',
        'ˡ' | 'ᴸ' => 'L',
        'ᵐ' | 'ᴹ' => 'M',
        'ⁿ' | 'ᴺ' => 'N',
        'ᵒ' | 'ᴼ' => 'O',
        'ᵖ' | 'ᴾ' => 'P',
        'ʳ' | 'ᴿ' => 'R',
        'ˢ' => 'S',
        'ᵗ' | 'ᵀ' => 'T',
        'ᵘ' | 'ᵁ' => 'U',
        'ᵛ' | 'ⱽ' => 'V',
        'ʷ' | 'ᵂ' => 'W',
        'ˣ' => 'X',
        'ʸ' => 'Y',
        'ᶻ' => 'Z',
        '⁰' => '0',
        '¹' => '1',
        '²' => '2',
        '³' => '3',
        '⁴' => '4',
        '⁵' => '5',
        '⁶' => '6',
        '⁷' => '7',
        '⁸' => '8',
        '⁹' => '9',
        '⁺' => '+',
        _ => return None,
    })
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

    /// The programmes whose title has `needle` in it (case and accents aside), on
    /// `channels` (by index, those `keep` lets through) that ended no earlier than
    /// `since`: those on at `now` first, then those to come by when they start,
    /// then those gone, the latest first. At most `limit` of them.
    pub fn search(
        &self,
        channels: &[Channel],
        keep: impl Fn(&Channel) -> bool,
        needle: &str,
        now: i64,
        since: i64,
        limit: usize,
    ) -> Vec<(usize, Programme)> {
        let needle = fold(needle.trim());
        if needle.is_empty() {
            return Vec::new();
        }
        // Channels sharing a guide id (HD and SD of one) show it once.
        let mut seen = std::collections::HashSet::new();
        let mut hits: Vec<(usize, &Programme)> = Vec::new();
        for (i, channel) in channels.iter().enumerate() {
            let Some(id) = channel.epg_id.as_deref() else { continue };
            if !keep(channel) || !seen.insert(id) {
                continue;
            }
            let list = self.for_channel(channel);
            let from = list.partition_point(|p| p.stop < since);
            hits.extend(list[from..].iter().filter(|p| has_folded(&p.title, &needle)).map(|p| (i, p)));
        }
        let rank = |p: &Programme| match () {
            _ if p.is_on(now) => (0, p.start),
            _ if p.start > now => (1, p.start),
            _ => (2, -p.start),
        };
        hits.sort_by_key(|(_, p)| rank(p));
        hits.into_iter().take(limit).map(|(i, p)| (i, p.clone())).collect()
    }
}

/// Whether `text` has `needle` (folded already) in it, folded. Most titles are
/// plain ASCII: those are matched as they are, without a folded copy.
fn has_folded(text: &str, needle: &str) -> bool {
    if text.is_ascii() {
        let (text, needle) = (text.as_bytes(), needle.as_bytes());
        needle.len() <= text.len() && text.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
    } else {
        fold(text).contains(needle)
    }
}

/// `text` for matching: lowercase, with the accents of the region's letters gone.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'č' | 'ć' | 'ç' => 'c',
            'š' => 's',
            'ž' => 'z',
            'đ' => 'd',
            'á' | 'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'ő' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'ű' => 'u',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programmes_by_title() {
        let channel = |name: &str, epg: &str| Channel {
            key: name.into(),
            name: name.into(),
            number: None,
            logo: None,
            group: "G".into(),
            url: String::new(),
            epg_id: Some(epg.into()),
            catchup: None,
            user_agent: None,
            referrer: None,
        };
        let prog = |start: i64, title: &str| Programme { start, stop: start + 100, title: title.into(), desc: String::new() };
        let channels = [channel("One", "1"), channel("One HD", "1"), channel("Two", "2"), channel("Hidden", "3")];
        let guide = Guide {
            programmes: HashMap::from([
                ("1".into(), vec![prog(0, "Old Dnevnik"), prog(800, "Dnevnik"), prog(1000, "Dnevnik"), prog(1200, "Film")]),
                ("2".into(), vec![prog(900, "Večernji DNEVNIK"), prog(1100, "Dnevnik 2")]),
                ("3".into(), vec![prog(1000, "Dnevnik")]),
            ]),
        };
        let found = guide.search(&channels, |c| c.name != "Hidden", "dnevnik", 950, 500, 10);
        let found: Vec<(usize, i64)> = found.iter().map(|(i, p)| (*i, p.start)).collect();
        // On now, then to come, then gone (the latest first); too old and hidden left out.
        assert_eq!(found, [(2, 900), (0, 1000), (2, 1100), (0, 800)]);
        assert!(guide.search(&channels, |_| true, "  ", 950, 0, 10).is_empty());
        assert_eq!(guide.search(&channels, |_| true, "vecernji", 950, 0, 10).len(), 1);
    }

    #[test]
    fn tidies_names() {
        assert_eq!(tidy_name("Arena Sport 1 ᶠʰᵈ"), "Arena Sport 1 FHD");
        assert_eq!(tidy_name("RTL ᴴᴰ+"), "RTL HD+");
        assert_eq!(tidy_name("Nova TVʰᵈ ʰᵉᵛᶜ"), "Nova TV HD HEVC");
        assert_eq!(tidy_name("ᵁᴴᴰ Cinema"), "UHD Cinema");
        assert_eq!(tidy_name("Zu\u{308}rich Cafe\u{301}"), "Zürich Café");
        assert_eq!(tidy_name("Area 5 m²"), "Area 5 m²");
        assert_eq!(tidy_name("HRT 1 HD (1080p) [Geo-blocked]"), "HRT 1 HD");
        assert_eq!(tidy_name("Klasik (576p) [Not 24/7]"), "Klasik");
        assert_eq!(tidy_name("OTV (Croatia) (720p)"), "OTV (Croatia)");
        assert_eq!(tidy_name("Channel (2024p) x"), "Channel (2024p) x");
    }

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
