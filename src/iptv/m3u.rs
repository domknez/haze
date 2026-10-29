//! Extended M3U playlists, as IPTV providers write them:
//!
//! ```text
//! #EXTM3U url-tvg="http://…/epg.xml.gz"
//! #EXTINF:-1 tvg-id="hrt1.hr" tvg-logo="http://…" group-title="Croatia" catchup="default" catchup-days="7",HRT 1
//! #EXTVLCOPT:http-user-agent=Mozilla/5.0
//! http://…/live/1.ts
//! ```
//!
//! Providers' playlists often carry their video library too. Where an entry goes:
//! 1. where the viewer put its group ([`parse_with`]);
//! 2. else, in playlists whose links don't say (no `…/movie/…` or `…/series/…`
//!    anywhere), where most of its group looks like it belongs: episode tags
//!    (`S1 E2`, `1x02`, `Season 1 Episode 2`) make a group series, years
//!    (`Dune (2021)`) and video links make it movies;
//! 3. else what its link says: `…/movie/…` and video files are movies, `…/series/…`
//!    episodes, anything else a channel.

use super::model::{Catalog, Catchup, CatchupKind, Channel, Episode, Movie, Series, Shelf};
use std::collections::{BTreeMap, HashMap};

/// The group of channels whose entry names none.
pub const NO_GROUP: &str = "Other";

/// Whether `text` is a playlist at all (not a web page or an error message where
/// one was expected): it starts with `#EXTM3U`, or its first line is a link.
pub fn looks_like_playlist(text: &str) -> bool {
    let first = text.trim_start_matches('\u{feff}').lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    first.starts_with("#EXTM3U") || first.starts_with("#EXTINF") || first.contains("://")
}

#[cfg(test)]
pub fn parse(text: &str) -> Catalog {
    parse_with(text, &BTreeMap::new())
}

/// A playlist, with the groups in `shelves` put where the viewer said they belong.
pub fn parse_with(text: &str, shelves: &BTreeMap<String, Shelf>) -> Catalog {
    let mut catalog = Catalog::default();
    let mut entry: Option<Entry> = None;
    let mut entries: Vec<(Entry, String)> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTM3U") {
            let attrs = attributes(rest);
            catalog.epg_url = ["url-tvg", "x-tvg-url", "tvg-url"]
                .iter()
                .find_map(|k| attr(&attrs, k))
                // Several guides can be listed, comma-separated; the first is used.
                .and_then(|v| v.split(',').map(str::trim).find(|u| !u.is_empty()).map(str::to_string));
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            entry = Some(Entry::from_extinf(rest));
        } else if let Some(group) = line.strip_prefix("#EXTGRP:") {
            if let Some(e) = &mut entry
                && e.group.is_none()
            {
                e.group = Some(group.trim().to_string());
            }
        } else if let Some(opt) = line.strip_prefix("#EXTVLCOPT:") {
            if let Some(e) = &mut entry {
                if let Some(ua) = opt.strip_prefix("http-user-agent=") {
                    e.user_agent = Some(ua.trim().to_string());
                } else if let Some(r) = opt.strip_prefix("http-referrer=") {
                    e.referrer = Some(r.trim().to_string());
                }
            }
        } else if line.starts_with('#') {
            // Other directives (#KODIPROP, #EXTBYT, comments) carry nothing we use.
        } else if entry.is_some() || line.contains("://") {
            // A bare URL without #EXTINF is still playable, named after itself.
            entries.push((entry.take().unwrap_or_default(), line.to_string()));
        }
    }
    // What each group's entries look like, counted by shelf.
    let mut looks: HashMap<String, [usize; 3]> = HashMap::new();
    for (e, url) in &entries {
        looks.entry(e.group()).or_default()[e.looks(url) as usize] += 1;
    }
    // Xtream panels' links say what each entry is, even where live channels and
    // films share a group name.
    let links_say = entries.iter().any(|(_, url)| library_path(url));
    let shelf_of: HashMap<String, Option<Shelf>> = looks
        .into_iter()
        .map(|(g, counts)| (g.clone(), shelves.get(&g).copied().or_else(|| if links_say { None } else { majority(counts) })))
        .collect();
    let mut series_at: HashMap<String, usize> = HashMap::new();
    for (e, url) in entries {
        let shelf = match shelf_of.get(&e.group()).copied().flatten() {
            // A tagged episode among movies is still an episode.
            Some(Shelf::Movies) if episode_tag(&e.name).is_some() => Shelf::Series,
            Some(shelf) => shelf,
            None => by_link(&url, &e.name),
        };
        match shelf {
            Shelf::Channels => catalog.channels.push(e.into_channel(&url)),
            Shelf::Movies => catalog.movies.push(e.into_movie(&url)),
            Shelf::Series => e.add_episode(&url, &mut catalog.series, &mut series_at),
        }
    }
    for s in &mut catalog.series {
        s.episodes.sort_by_key(|e| (e.season, e.number));
    }
    catalog.groups = Catalog::groups_of(&catalog.channels);
    catalog.movie_groups = Catalog::groups_of_movies(&catalog.movies);
    catalog.series_groups = Catalog::groups_of_series(&catalog.series);
    catalog
}

/// Where a whole group goes when most of it (and at least three entries) looks the
/// same way; none leaves each entry to its link.
fn majority([channels, movies, series]: [usize; 3]) -> Option<Shelf> {
    let n = channels + movies + series;
    let most = |k: usize| n >= 3 && k * 10 >= n * 6;
    if most(series) {
        Some(Shelf::Series)
    } else if most(movies + series) {
        Some(Shelf::Movies)
    } else {
        None
    }
}

/// The path of a link, without the server, query and fragment.
fn path_of(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p))
}

/// An Xtream panel's link into the video library.
fn library_path(url: &str) -> bool {
    let path = path_of(url);
    path.starts_with("movie/") || path.starts_with("series/")
}

/// Where an entry goes by its link (Xtream panels' `/movie/` and `/series/` paths,
/// or a video file) and its name.
fn by_link(url: &str, name: &str) -> Shelf {
    let path = path_of(url);
    if path.starts_with("series/") {
        return Shelf::Series;
    }
    let file = path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    let video_file = file.is_some_and(|ext| matches!(ext.as_str(), "mkv" | "mp4" | "m4v" | "avi" | "mov" | "webm"));
    if path.starts_with("movie/") || video_file {
        if episode_tag(name).is_some() { Shelf::Series } else { Shelf::Movies }
    } else {
        Shelf::Channels
    }
}

/// An episode tag in a name: `Show Name S1 E2 Title` → (`Show Name`, 1, 2, `Title`).
/// Also `S01E02`, `s1.e2`, `1x02`, `Season 1 Episode 2`, `Episode 5` and `Ep. 5`
/// (season 1), and the same in a few other languages.
fn episode_tag(name: &str) -> Option<(&str, u32, u32, &str)> {
    let low = name.to_ascii_lowercase();
    let b = low.as_bytes();
    let edge = |c: char| c.is_whitespace() || "-–:|.,#".contains(c);
    (0..b.len()).filter(|&i| i == 0 || !b[i - 1].is_ascii_alphanumeric()).find_map(|i| {
        let (season, number, end) = tag_at(b, i)?;
        Some((name[..i].trim_end_matches(edge), season, number, name[end..].trim_start_matches(edge)))
    })
}

/// A season and episode tag starting at `i`, and where it ends.
fn tag_at(b: &[u8], i: usize) -> Option<(u32, u32, usize)> {
    const SEASON: &[&str] = &["season", "staffel", "saison", "sezona", "temporada", "stagione"];
    const EPISODE: &[&str] = &["episode", "episodio", "epizoda", "folge", "ep"];
    if let Some(j) = word(b, i, SEASON) {
        let (s, j) = digits(b, skip(b, j), 3)?;
        let j = word(b, skip(b, j), EPISODE)?;
        let (e, j) = digits(b, skip(b, j), 4)?;
        return Some((s, e, j));
    }
    if let Some(j) = word(b, i, EPISODE) {
        let (e, j) = digits(b, skip(b, j), 4)?;
        return Some((1, e, j));
    }
    if b[i] == b's' {
        let (s, j) = digits(b, i + 1, 3)?;
        let j = skip(b, j);
        if b.get(j) != Some(&b'e') {
            return None;
        }
        let (e, j) = digits(b, j + 1, 4)?;
        return Some((s, e, j));
    }
    // `1x02` after a name (a film can be called `4x4`); at most two digits before
    // the x, so `1920x1080` isn't one.
    if i == 0 {
        return None;
    }
    let (s, j) = digits(b, i, 2)?;
    if b.get(j) != Some(&b'x') {
        return None;
    }
    let (e, j) = digits(b, j + 1, 3)?;
    Some((s, e, j))
}

/// The number of at most `max` digits at `j`, and where it ends.
fn digits(b: &[u8], j: usize, max: usize) -> Option<(u32, usize)> {
    let k = j + b.get(j..)?.iter().take_while(|c| c.is_ascii_digit()).count();
    if k == j || k - j > max {
        return None;
    }
    Some((std::str::from_utf8(&b[j..k]).ok()?.parse().ok()?, k))
}

/// Past spaces and the separators between a tag's parts.
fn skip(b: &[u8], mut j: usize) -> usize {
    while j < b.len() && matches!(b[j], b' ' | b'.' | b'-' | b'_' | b',' | b':') {
        j += 1;
    }
    j
}

/// Past one of `words` at `j`.
fn word(b: &[u8], j: usize, words: &[&str]) -> Option<usize> {
    words.iter().find(|w| b[j..].starts_with(w.as_bytes())).map(|w| j + w.len())
}

/// `Show 05` → (`Show`, 5): a trailing number, for episodes without a tag.
fn trailing_number(name: &str) -> (&str, Option<u32>) {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let head = &name[..name.len() - digits];
    if digits == 0 || digits > 4 || !head.ends_with(|c: char| c.is_whitespace() || "-–:|.,#".contains(c)) {
        return (name, None);
    }
    (head.trim_end_matches(|c: char| c.is_whitespace() || "-–:|.,#".contains(c)), name[head.len()..].parse().ok())
}

#[derive(Default)]
struct Entry {
    name: String,
    attrs: Vec<(String, String)>,
    group: Option<String>,
    user_agent: Option<String>,
    referrer: Option<String>,
}

impl Entry {
    /// `-1 key="value" …,Display name`
    fn from_extinf(rest: &str) -> Entry {
        let (head, name) = split_name(rest);
        let attrs = attributes(head);
        let group = attr(&attrs, "group-title").map(str::to_string);
        Entry { name: super::model::tidy_name(name), attrs, group, ..Entry::default() }
    }

    fn into_channel(self, url: &str) -> Channel {
        let get = |k| attr(&self.attrs, k).map(str::to_string);
        let name = if self.name.is_empty() { get("tvg-name").unwrap_or_else(|| url.to_string()) } else { self.name.clone() };
        Channel {
            key: url.to_string(),
            number: get("tvg-chno").or_else(|| get("channel-number")).and_then(|n| n.parse().ok()),
            logo: get("tvg-logo"),
            group: self.group(),
            url: url.to_string(),
            epg_id: get("tvg-id").or_else(|| get("tvg-name")),
            catchup: catchup(&self.attrs),
            user_agent: self.user_agent.clone().or_else(|| get("http-user-agent")),
            referrer: self.referrer.clone().or_else(|| get("http-referrer")),
            name,
        }
    }
}

impl Entry {
    fn group(&self) -> String {
        self.group.clone().filter(|g| !g.is_empty()).unwrap_or_else(|| NO_GROUP.to_string())
    }

    fn into_movie(self, url: &str) -> Movie {
        let name = if self.name.is_empty() { attr(&self.attrs, "tvg-name").unwrap_or(url).to_string() } else { self.name.clone() };
        Movie {
            key: url.to_string(),
            year: super::xtream::year_in(&name),
            poster: attr(&self.attrs, "tvg-logo").map(str::to_string),
            group: self.group(),
            url: url.to_string(),
            rating: None,
            name,
        }
    }

    /// Where the entry looks like it belongs, by its link and its name.
    fn looks(&self, url: &str) -> Shelf {
        match by_link(url, &self.name) {
            Shelf::Channels if episode_tag(&self.name).is_some() => Shelf::Series,
            Shelf::Channels if super::xtream::year_in(&self.name).is_some() => Shelf::Movies,
            shelf => shelf,
        }
    }

    fn add_episode(self, url: &str, series: &mut Vec<Series>, at: &mut HashMap<String, usize>) {
        let group = self.group();
        let (show, season, number, title) = match episode_tag(&self.name) {
            // `S1 E2 Pilot` names only the episode: the group is the series.
            Some(("", s, n, t)) => (group.as_str(), s, Some(n), t),
            Some((show, s, n, t)) => (show, s, Some(n), t),
            None => {
                let (show, n) = trailing_number(&self.name);
                (show, 1, n, "")
            }
        };
        let key = format!("{group}/{show}");
        let i = *at.entry(key.clone()).or_insert_with(|| {
            series.push(Series {
                key,
                name: show.to_string(),
                poster: None,
                year: super::xtream::year_in(show),
                group: group.clone(),
                rating: None,
                plot: None,
                episodes: Vec::new(),
            });
            series.len() - 1
        });
        let s = &mut series[i];
        if s.poster.is_none() {
            s.poster = attr(&self.attrs, "tvg-logo").map(str::to_string);
        }
        let number = number.unwrap_or(s.episodes.len() as u32 + 1);
        s.episodes.push(Episode { key: url.to_string(), season, number, title: title.to_string(), url: url.to_string() });
    }
}

fn catchup(attrs: &[(String, String)]) -> Option<Catchup> {
    let mode = attr(attrs, "catchup").or_else(|| attr(attrs, "catchup-type"))?.to_ascii_lowercase();
    let days = attr(attrs, "catchup-days").or_else(|| attr(attrs, "timeshift")).and_then(|d| d.parse().ok()).unwrap_or(1);
    let source = attr(attrs, "catchup-source").map(str::to_string);
    let kind = match (mode.as_str(), source) {
        ("default", Some(source)) => CatchupKind::Template { source, append: false },
        ("append", Some(source)) => CatchupKind::Template { source, append: true },
        ("shift" | "timeshift" | "default" | "append", None) => CatchupKind::Shift,
        ("flussonic" | "flussonic-hls" | "fs", _) => CatchupKind::Flussonic,
        _ => return None,
    };
    Some(Catchup { days, kind })
}

/// Splits `attrs,Name` at the comma that ends the attributes, ignoring commas
/// inside quoted values.
fn split_name(s: &str) -> (&str, &str) {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => return (&s[..i], &s[i + 1..]),
            _ => {}
        }
    }
    (s, "")
}

/// `key="value" key2=value2` pairs; keys lowercased.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].rsplit(|c: char| c.is_whitespace()).next().unwrap_or("").trim().to_ascii_lowercase();
        let after = &rest[eq + 1..];
        let (value, next) = if let Some(q) = after.strip_prefix('"') {
            match q.find('"') {
                Some(end) => (&q[..end], &q[end + 1..]),
                None => (q, ""),
            }
        } else {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if !key.is_empty() {
            out.push((key, value.to_string()));
        }
        rest = next;
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"#EXTM3U url-tvg="http://epg.example/guide.xml.gz, http://other/epg.xml"
#EXTINF:-1 tvg-id="hrt1.hr" tvg-name="HRT 1" tvg-logo="http://logo/hrt1.png" group-title="Croatia" tvg-chno="1" catchup="default" catchup-days="7" catchup-source="http://cdn/hrt1?start={utc}&dur={duration}",HRT 1 HD
http://example/live/hrt1.ts

#EXTINF:-1 tvg-id="" group-title="Sports, Live",Arena Sport 1
#EXTVLCOPT:http-user-agent=VLC/3.0
#EXTVLCOPT:http-referrer=http://example/
http://example/live/arena1.m3u8
#EXTINF:0,No group
#EXTGRP:Music
http://example/live/music.ts
http://example/bare.ts
"#;

    #[test]
    fn reads_channels_and_attributes() {
        let c = parse(LIST);
        assert_eq!(c.epg_url.as_deref(), Some("http://epg.example/guide.xml.gz"));
        assert_eq!(c.channels.len(), 4);
        let hrt = &c.channels[0];
        assert_eq!(hrt.name, "HRT 1 HD");
        assert_eq!(hrt.number, Some(1));
        assert_eq!(hrt.epg_id.as_deref(), Some("hrt1.hr"));
        assert_eq!(hrt.logo.as_deref(), Some("http://logo/hrt1.png"));
        assert_eq!(hrt.group, "Croatia");
        assert_eq!(
            hrt.catchup,
            Some(Catchup {
                days: 7,
                kind: CatchupKind::Template { source: "http://cdn/hrt1?start={utc}&dur={duration}".into(), append: false }
            })
        );

        let arena = &c.channels[1];
        assert_eq!(arena.name, "Arena Sport 1");
        assert_eq!(arena.group, "Sports, Live", "comma inside a quoted value");
        assert_eq!(arena.epg_id, None);
        assert_eq!(arena.user_agent.as_deref(), Some("VLC/3.0"));
        assert_eq!(arena.referrer.as_deref(), Some("http://example/"));
        assert_eq!(arena.url, "http://example/live/arena1.m3u8");

        assert_eq!(c.channels[2].group, "Music", "from #EXTGRP");
        assert_eq!(c.channels[3].name, "http://example/bare.ts");
        assert_eq!(c.channels[3].group, NO_GROUP);
        assert_eq!(c.groups, ["Croatia", "Sports, Live", "Music", NO_GROUP]);
    }

    #[test]
    fn sorts_out_the_video_library() {
        let c = parse(
            r#"#EXTM3U
#EXTINF:-1 tvg-logo="http://logo/hrt1.png" group-title="Croatia",HRT 1
http://line.tv:80/user/pass/1
#EXTINF:-1 tvg-logo="http://img/dune.jpg" group-title="Films 2021",Dune (2021)
http://line.tv:80/movie/user/pass/5.mkv
#EXTINF:-1 tvg-logo="http://img/dark.jpg" group-title="Drama",Dark S2 E1
http://line.tv:80/series/user/pass/202.mkv
#EXTINF:-1 group-title="Drama",Dark S1 E10
http://line.tv:80/series/user/pass/110.mkv
#EXTINF:-1 group-title="Drama",Dark S01E02 Lies
http://line.tv:80/series/user/pass/102.mkv
#EXTINF:-1 group-title="Home",Holiday.mp4
http://nas.local/videos/holiday.mp4
"#,
        );
        assert_eq!(c.channels.len(), 1);
        assert_eq!(c.groups, ["Croatia"]);
        let names: Vec<_> = c.movies.iter().map(|m| (m.name.as_str(), m.year)).collect();
        assert_eq!(names, [("Dune (2021)", Some(2021)), ("Holiday.mp4", None)]);
        assert_eq!(c.movie_groups, ["Films 2021", "Home"]);
        assert_eq!(c.series.len(), 1);
        let dark = &c.series[0];
        assert_eq!((dark.name.as_str(), dark.poster.as_deref()), ("Dark", Some("http://img/dark.jpg")));
        let eps: Vec<_> = dark.episodes.iter().map(|e| (e.code(), e.title.as_str())).collect();
        assert_eq!(eps, [("S1 E2".into(), "Lies"), ("S1 E10".into(), ""), ("S2 E1".into(), "")]);
    }

    #[test]
    fn whole_groups_follow_what_most_of_them_look_like() {
        let c = parse(
            r#"#EXTM3U
#EXTINF:-1 group-title="VOD | Action",Heat (1995)
http://cdn.tv/vod/1.m3u8
#EXTINF:-1 group-title="VOD | Action",Ronin (1998)
http://cdn.tv/vod/2.m3u8
#EXTINF:-1 group-title="VOD | Action",Collateral
http://cdn.tv/vod/3.m3u8
#EXTINF:-1 group-title="Shows",Lost 1x01
http://cdn.tv/s/1.m3u8
#EXTINF:-1 group-title="Shows",Lost 1x02
http://cdn.tv/s/2.m3u8
#EXTINF:-1 group-title="Shows",Lost 1x03
http://cdn.tv/s/3.m3u8
#EXTINF:-1 group-title="Cinema",Cinema 1
http://cdn.tv/live/c1.m3u8
#EXTINF:-1 group-title="Cinema",Cinema 2
http://cdn.tv/live/c2.m3u8
#EXTINF:-1 group-title="Cinema",Classics (1950s)
http://cdn.tv/live/c3.m3u8
"#,
        );
        let movies: Vec<_> = c.movies.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(movies, ["Heat (1995)", "Ronin (1998)", "Collateral"], "the whole group, with a year or not");
        assert_eq!(c.channels.len(), 3, "a few odd names don't move a group of channels");
        let series: Vec<_> = c.series.iter().map(|s| (s.name.as_str(), s.episodes.len())).collect();
        assert_eq!(series, [("Lost", 3)]);
        let c = parse("#EXTINF:-1,Tom 01\nhttp://cdn.tv/series/u/p/1.mp4\n#EXTINF:-1,Tom 02\nhttp://cdn.tv/series/u/p/2.mp4\n");
        let series: Vec<_> = c.series.iter().map(|s| (s.name.as_str(), s.episodes.len())).collect();
        assert_eq!(series, [("Tom", 2)], "untagged episodes are gathered by name");

        let shelves = BTreeMap::from([("Cinema".to_string(), Shelf::Movies), ("VOD | Action".to_string(), Shelf::Channels)]);
        let c = parse_with(&c_text(), &shelves);
        assert_eq!((c.movies.len(), c.channels.len()), (3, 3), "the viewer's choice wins");
    }

    fn c_text() -> String {
        ["VOD | Action", "VOD | Action", "VOD | Action", "Cinema", "Cinema", "Cinema"]
            .iter()
            .enumerate()
            .map(|(i, g)| format!("#EXTINF:-1 group-title=\"{g}\",Item ({})\nhttp://cdn.tv/{i}.m3u8\n", 2000 + i))
            .collect()
    }

    #[test]
    fn links_decide_where_they_say() {
        let c = parse(
            r#"#EXTM3U
#EXTINF:-1 group-title="Kids",Toon One
http://line.tv/u/p/1
#EXTINF:-1 group-title="Kids",Bluey S1 E1
http://line.tv/series/u/p/11.mkv
#EXTINF:-1 group-title="Kids",Bluey S1 E2
http://line.tv/series/u/p/12.mkv
#EXTINF:-1 group-title="Kids",Bluey S1 E3
http://line.tv/series/u/p/13.mkv
"#,
        );
        assert_eq!((c.channels.len(), c.series.len()), (1, 1), "a channel sharing a group name with a series stays");
    }

    #[test]
    fn reads_episode_tags() {
        assert_eq!(episode_tag("7 Bears S1 E3"), Some(("7 Bears", 1, 3, "")));
        assert_eq!(episode_tag("Dark - s02.e05 - Lost"), Some(("Dark", 2, 5, "Lost")));
        assert_eq!(episode_tag("Sons S3E12"), Some(("Sons", 3, 12, "")));
        assert_eq!(episode_tag("Friends 2x05 The One"), Some(("Friends", 2, 5, "The One")));
        assert_eq!(episode_tag("Lost - Season 3 Episode 7"), Some(("Lost", 3, 7, "")));
        assert_eq!(episode_tag("Dark Staffel 1 Folge 4"), Some(("Dark", 1, 4, "")));
        assert_eq!(episode_tag("Vikings Ep. 12"), Some(("Vikings", 1, 12, "")));
        assert_eq!(episode_tag("Movie 1920x1080"), None);
        assert_eq!(episode_tag("4x4"), None);
        assert_eq!(episode_tag("Epic Sports 2"), None);
        assert_eq!(episode_tag("Superman Returns"), None);
        assert_eq!(episode_tag("SES Astra"), None);
    }

    #[test]
    fn tolerates_odd_attributes() {
        let a = attributes(r#"-1 tvg-ID=abc  group-title="x y" broken="unterminated"#);
        assert_eq!(attr(&a, "tvg-id"), Some("abc"));
        assert_eq!(attr(&a, "group-title"), Some("x y"));
        assert_eq!(attr(&a, "broken"), Some("unterminated"));
        assert_eq!(parse("").channels.len(), 0);
        assert_eq!(parse("not a link\n#EXTINF:-1,A\nhttp://a\n").channels.len(), 1, "stray text is skipped");
        assert!(!looks_like_playlist("<!DOCTYPE html>\n<html>"));
        assert!(looks_like_playlist("\u{feff}#EXTM3U\n") && looks_like_playlist("http://a/1.ts\n"));
        assert_eq!(parse("\u{feff}#EXTM3U\r\n#EXTINF:-1,A\r\nhttp://a\r\n").channels[0].name, "A");
    }
}
