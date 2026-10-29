//! Extended M3U playlists, as IPTV providers write them:
//!
//! ```text
//! #EXTM3U url-tvg="http://…/epg.xml.gz"
//! #EXTINF:-1 tvg-id="hrt1.hr" tvg-logo="http://…" group-title="Croatia" catchup="default" catchup-days="7",HRT 1
//! #EXTVLCOPT:http-user-agent=Mozilla/5.0
//! http://…/live/1.ts
//! ```

use super::model::{Catalog, Catchup, CatchupKind, Channel};

/// The group of channels whose entry names none.
pub const NO_GROUP: &str = "Other";

pub fn parse(text: &str) -> Catalog {
    let mut catalog = Catalog::default();
    let mut entry: Option<Entry> = None;
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
        } else if let Some(e) = entry.take() {
            catalog.channels.push(e.into_channel(line));
        } else {
            // A bare URL without #EXTINF: still playable, named after itself.
            catalog.channels.push(Entry::default().into_channel(line));
        }
    }
    catalog.groups = Catalog::groups_of(&catalog.channels);
    catalog
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
            group: self.group.clone().filter(|g| !g.is_empty()).unwrap_or_else(|| NO_GROUP.to_string()),
            url: url.to_string(),
            epg_id: get("tvg-id").or_else(|| get("tvg-name")),
            catchup: catchup(&self.attrs),
            user_agent: self.user_agent.clone().or_else(|| get("http-user-agent")),
            referrer: self.referrer.clone().or_else(|| get("http-referrer")),
            name,
        }
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
    fn tolerates_odd_attributes() {
        let a = attributes(r#"-1 tvg-ID=abc  group-title="x y" broken="unterminated"#);
        assert_eq!(attr(&a, "tvg-id"), Some("abc"));
        assert_eq!(attr(&a, "group-title"), Some("x y"));
        assert_eq!(attr(&a, "broken"), Some("unterminated"));
        assert_eq!(parse("").channels.len(), 0);
        assert_eq!(parse("\u{feff}#EXTM3U\r\n#EXTINF:-1,A\r\nhttp://a\r\n").channels[0].name, "A");
    }
}
