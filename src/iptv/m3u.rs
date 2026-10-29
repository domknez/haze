//! Extended M3U playlists, as IPTV providers write them:
//!
//! ```text
//! #EXTM3U url-tvg="http://…/epg.xml.gz"
//! #EXTINF:-1 tvg-id="hrt1.hr" tvg-logo="http://…" group-title="Croatia" catchup="default" catchup-days="7",HRT 1
//! #EXTVLCOPT:http-user-agent=Mozilla/5.0
//! http://…/live/1.ts
//! ```
//!
//! Providers' playlists often carry their video library too: entries linking to
//! `…/movie/…` are movies, and `…/series/…` ones episodes, named `Show S1 E2`.

use super::model::{Catalog, Catchup, CatchupKind, Channel, Episode, Movie, Series};
use std::collections::HashMap;

/// The group of channels whose entry names none.
pub const NO_GROUP: &str = "Other";

/// Whether `text` is a playlist at all (not a web page or an error message where
/// one was expected): it starts with `#EXTM3U`, or its first line is a link.
pub fn looks_like_playlist(text: &str) -> bool {
    let first = text.trim_start_matches('\u{feff}').lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    first.starts_with("#EXTM3U") || first.starts_with("#EXTINF") || first.contains("://")
}

pub fn parse(text: &str) -> Catalog {
    let mut catalog = Catalog::default();
    let mut entry: Option<Entry> = None;
    // Each series' place in `catalog.series`, by its key.
    let mut series_at: HashMap<String, usize> = HashMap::new();
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
            let e = entry.take().unwrap_or_default();
            match kind_of(line, &e.name) {
                Kind::Live => catalog.channels.push(e.into_channel(line)),
                Kind::Movie => catalog.movies.push(e.into_movie(line)),
                Kind::Episode => e.add_episode(line, &mut catalog.series, &mut series_at),
            }
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

enum Kind {
    Live,
    Movie,
    Episode,
}

/// What an entry is, by its link (Xtream panels' `/movie/` and `/series/` paths,
/// or a video file) and its name.
fn kind_of(url: &str, name: &str) -> Kind {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p));
    if path.starts_with("series/") {
        return Kind::Episode;
    }
    let file = path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    let video_file = file.is_some_and(|ext| matches!(ext.as_str(), "mkv" | "mp4" | "m4v" | "avi" | "mov" | "webm"));
    if path.starts_with("movie/") || video_file {
        if episode_tag(name).is_some() { Kind::Episode } else { Kind::Movie }
    } else {
        Kind::Live
    }
}

/// `Show Name S1 E2 Title` → (`Show Name`, 1, 2, `Title`); also `S01E02`, `s1.e2`.
fn episode_tag(name: &str) -> Option<(&str, u32, u32, &str)> {
    let b = name.as_bytes();
    let digits = |mut j: usize| {
        let from = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        (from, j)
    };
    let edge = |c: char| c.is_whitespace() || "-–:|.,".contains(c);
    for i in 0..b.len() {
        if !matches!(b[i], b'S' | b's') || i > 0 && b[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let (s0, s1) = digits(i + 1);
        if s1 == s0 || s1 - s0 > 3 {
            continue;
        }
        let mut j = s1;
        while j < b.len() && matches!(b[j], b' ' | b'.' | b'-' | b'_') {
            j += 1;
        }
        if j >= b.len() || !matches!(b[j], b'E' | b'e') {
            continue;
        }
        let (e0, end) = digits(j + 1);
        if end == e0 || end - e0 > 4 || end < b.len() && b[end].is_ascii_alphabetic() {
            continue;
        }
        let (Ok(season), Ok(number)) = (name[s0..s1].parse(), name[e0..end].parse()) else {
            continue;
        };
        return Some((name[..i].trim_end_matches(edge), season, number, name[end..].trim_start_matches(edge)));
    }
    None
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
        Entry { name: name.trim().to_string(), attrs, group, ..Entry::default() }
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

    fn add_episode(self, url: &str, series: &mut Vec<Series>, at: &mut HashMap<String, usize>) {
        let (show, season, number, title) = match episode_tag(&self.name) {
            Some((show, s, n, t)) if !show.is_empty() => (show, s, n, t),
            _ => (self.name.as_str(), 1, 0, ""),
        };
        let group = self.group();
        let key = format!("{group}/{show}");
        let i = *at.entry(key.clone()).or_insert_with(|| {
            series.push(Series {
                key,
                name: show.to_string(),
                poster: None,
                year: super::xtream::year_in(show),
                group,
                rating: None,
                episodes: Vec::new(),
            });
            series.len() - 1
        });
        let s = &mut series[i];
        if s.poster.is_none() {
            s.poster = attr(&self.attrs, "tvg-logo").map(str::to_string);
        }
        let number = if number == 0 { s.episodes.len() as u32 + 1 } else { number };
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
    fn reads_episode_tags() {
        assert_eq!(episode_tag("7 Bears S1 E3"), Some(("7 Bears", 1, 3, "")));
        assert_eq!(episode_tag("Dark - s02.e05 - Lost"), Some(("Dark", 2, 5, "Lost")));
        assert_eq!(episode_tag("Sons S3E12"), Some(("Sons", 3, 12, "")));
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
