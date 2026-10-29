//! The viewer's sources (saved in `sources.json`, passwords in the keychain), loading
//! what each offers, and favourites and recently watched channels.

use crate::credentials;
use crate::iptv::model::{Catalog, CatchupKind, Channel, Guide, Programme, Shelf};
use crate::iptv::{http, m3u, time, xmltv, xtream};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Playlist groups the viewer put elsewhere (channels, movies, series).
    pub shelves: BTreeMap<String, Shelf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// An M3U playlist at a link or in a file, with an optional guide.
    Playlist { location: String, epg: Option<String> },
    /// An Xtream Codes login; the password is in the keychain.
    Xtream { server: String, user: String },
}

impl Source {
    /// A short tag for lists: `M3U` or `XC`.
    pub fn tag(&self) -> &'static str {
        match self.kind {
            Kind::Playlist { .. } => "M3U",
            Kind::Xtream { .. } => "XC",
        }
    }

    /// Where it comes from, for a second line.
    pub fn detail(&self) -> String {
        match &self.kind {
            Kind::Playlist { location, .. } if is_url(location) => format!("m3u · {}", host(location)),
            Kind::Playlist { location, .. } => format!("m3u · {}", location.rsplit('/').next().unwrap_or(location)),
            Kind::Xtream { server, user } => format!("xtream · {user}@{}", host(server)),
        }
    }

    pub fn login(&self) -> Option<xtream::Login> {
        match &self.kind {
            Kind::Xtream { server, user } => Some(xtream::Login::new(server, user, &credentials::load(&self.id).unwrap_or_default())),
            Kind::Playlist { .. } => None,
        }
    }

    /// The same source as an Xtream Codes login, when it's the playlist link of an
    /// Xtream panel (`…/get.php?username=…&password=…`): the panel's API gives posters,
    /// plots and seasons too, and the password can go to the keychain.
    pub fn as_xtream(&self) -> Option<(Source, xtream::Login)> {
        let Kind::Playlist { location, .. } = &self.kind else { return None };
        let login = xtream::Login::from_playlist_link(location)?;
        let kind = Kind::Xtream { server: login.server.clone(), user: login.user.clone() };
        Some((Source { kind, shelves: BTreeMap::new(), ..self.clone() }, login))
    }

    fn to_json(&self) -> Value {
        let mut v = match &self.kind {
            Kind::Playlist { location, epg } => {
                json!({"id": self.id, "name": self.name, "kind": "playlist", "location": location, "epg": epg})
            }
            Kind::Xtream { server, user } => json!({"id": self.id, "name": self.name, "kind": "xtream", "server": server, "user": user}),
        };
        if !self.shelves.is_empty() {
            v["shelves"] = self.shelves.iter().map(|(g, s)| (g.clone(), Value::from(s.key()))).collect::<serde_json::Map<_, _>>().into();
        }
        v
    }

    fn from_json(v: &Value) -> Option<Source> {
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let kind = match v["kind"].as_str()? {
            "playlist" => Kind::Playlist { location: s("location")?, epg: s("epg") },
            "xtream" => Kind::Xtream { server: s("server")?, user: s("user")? },
            _ => return None,
        };
        let shelves =
            v["shelves"].as_object().into_iter().flatten().filter_map(|(g, s)| Some((g.clone(), Shelf::from_key(s.as_str()?)?))).collect();
        Some(Source { id: s("id")?, name: s("name").unwrap_or_else(|| "Source".into()), kind, shelves })
    }
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

pub fn new_id() -> String {
    format!("{:08x}", fastrand::u32(..))
}

fn sources_file() -> Option<PathBuf> {
    crate::settings::dir().map(|d| d.join("sources.json"))
}

pub fn load_sources() -> Vec<Source> {
    let Some(text) = sources_file().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    v.as_array().into_iter().flatten().filter_map(Source::from_json).collect()
}

pub fn save_sources(sources: &[Source]) {
    let Some(p) = sources_file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let text = serde_json::to_string_pretty(&sources.iter().map(Source::to_json).collect::<Vec<_>>()).unwrap_or_default();
    if let Err(e) = std::fs::write(&p, text) {
        log_warn!("could not save sources to {}: {e}", p.display());
    }
}

/// Everything a source offers.
pub fn fetch(source: &Source) -> Result<Catalog, String> {
    let started = std::time::Instant::now();
    let catalog = match &source.kind {
        Kind::Playlist { location, epg } => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut http::open(location).map_err(|e| e.to_string())?, &mut text).map_err(|e| e.to_string())?;
            if !m3u::looks_like_playlist(&text) {
                return Err("that address gives a web page or other text, not a playlist".into());
            }
            let mut c = m3u::parse_with(&text, &source.shelves);
            if c.channels.is_empty() && c.movies.is_empty() && c.series.is_empty() {
                return Err("the playlist has no channels".into());
            }
            if epg.is_some() {
                c.epg_url = epg.clone();
            }
            c
        }
        Kind::Xtream { .. } => source.login().expect("xtream login").catalog().map_err(|e| e.to_string())?,
    };
    log_catalog(&source.name, &catalog, started);
    Ok(catalog)
}

/// A source signed in to the Xtream panel its playlist link comes from, with the
/// login's password: keep both in place of the link.
pub type Upgrade = (Source, String);

/// What a source offers, from the panel's API in place of its playlist link when
/// the source is one and the API answers (see [`Source::as_xtream`]); the source
/// as a login then comes too. A panel that keeps its API closed still gives the
/// playlist.
pub fn fetch_upgrading(source: &Source) -> Result<(Catalog, Option<Upgrade>), String> {
    if let Some((login_source, login)) = source.as_xtream() {
        let started = std::time::Instant::now();
        match login.catalog() {
            Ok(catalog) => {
                log_catalog(&source.name, &catalog, started);
                return Ok((catalog, Some((login_source, login.password))));
            }
            Err(e) => log_warn!("{}: the panel's API didn't answer ({e}); reading the playlist", source.name),
        }
    }
    Ok((fetch(source)?, None))
}

fn log_catalog(name: &str, catalog: &Catalog, started: std::time::Instant) {
    log_info!(
        "{name}: {} channels in {} groups, {} movies, {} series ({:.1}s)",
        catalog.channels.len(),
        catalog.groups.len(),
        catalog.movies.len(),
        catalog.series.len(),
        started.elapsed().as_secs_f32()
    );
}

/// The guide for `channels`.
pub fn fetch_guide(url: &str, channels: &[Channel]) -> Result<Guide, String> {
    let started = std::time::Instant::now();
    let ids: HashSet<String> = channels.iter().filter_map(|c| c.epg_id.clone()).collect();
    let reader = http::open(url).map_err(|e| e.to_string())?;
    let guide = xmltv::parse(reader, Some(&ids), time::now()).map_err(|e| e.to_string())?;
    log_info!("guide: {} channels ({:.1}s)", guide.programmes.len(), started.elapsed().as_secs_f32());
    Ok(guide)
}

/// The URL that plays `programme` from the archive, if the channel has one that
/// reaches back that far.
pub fn catchup_url(channel: &Channel, programme: &Programme, login: Option<&xtream::Login>, server_offset: i64) -> Option<String> {
    let catchup = channel.catchup.as_ref()?;
    let now = time::now();
    if programme.start > now || programme.start < now - i64::from(catchup.days) * 86_400 {
        return None;
    }
    let duration = (programme.stop.min(now) - programme.start).max(60);
    Some(match &catchup.kind {
        CatchupKind::Xtream { stream_id } => login?.timeshift_url(*stream_id, programme.start, (duration / 60) as u32 + 1, server_offset),
        CatchupKind::Template { source, append } => {
            let filled = fill_template(source, programme.start, programme.stop, now);
            if *append { format!("{}{filled}", channel.url) } else { filled }
        }
        CatchupKind::Shift => {
            let sep = if channel.url.contains('?') { '&' } else { '?' };
            format!("{}{sep}utc={}&lutc={now}", channel.url, programme.start)
        }
        CatchupKind::Flussonic => {
            let (base, _) = channel.url.rsplit_once('/')?;
            format!("{base}/index-{}-{duration}.m3u8", programme.start)
        }
    })
}

/// Fills a `catchup-source` template (the placeholders Kodi's IPTV Simple Client
/// understands).
fn fill_template(t: &str, start: i64, stop: i64, now: i64) -> String {
    let c = time::civil(start);
    let duration = stop - start;
    let mut s = t.to_string();
    for (k, v) in [
        ("{utc}", start.to_string()),
        ("${start}", start.to_string()),
        ("{start}", start.to_string()),
        ("${timestamp}", now.to_string()),
        ("{utcend}", stop.to_string()),
        ("${end}", stop.to_string()),
        ("{end}", stop.to_string()),
        ("{lutc}", now.to_string()),
        ("${now}", now.to_string()),
        ("{now}", now.to_string()),
        ("${offset}", (now - start).to_string()),
        ("${duration}", duration.to_string()),
        ("{duration}", duration.to_string()),
        ("{Y}", format!("{:04}", c.year)),
        ("{m}", format!("{:02}", c.month)),
        ("{d}", format!("{:02}", c.day)),
        ("{H}", format!("{:02}", c.hour)),
        ("{M}", format!("{:02}", c.minute)),
        ("{S}", format!("{:02}", c.second)),
    ] {
        s = s.replace(k, &v);
    }
    // `{duration:60}` (in minutes), `{offset:1}` (seconds back from now, divided).
    for (name, value) in [("duration", duration), ("offset", now - start)] {
        while let Some(i) = s.find(&format!("{{{name}:")) {
            let Some(end) = s[i..].find('}') else { break };
            let divisor: i64 = s[i + name.len() + 2..i + end].parse().unwrap_or(1).max(1);
            s.replace_range(i..i + end + 1, &(value / divisor).to_string());
        }
    }
    s
}

/// Favourite and recently watched channels, by source id and channel key.
#[derive(Default)]
pub struct Marks {
    favorites: Vec<(String, String)>,
    recent: Vec<(String, String)>,
    file: Option<PathBuf>,
}

/// Recently watched channels kept per source.
const RECENT: usize = 30;

impl Marks {
    pub fn load() -> Marks {
        let file = crate::settings::dir().map(|d| d.join("marks.json"));
        let v: Value =
            file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let pairs = |k: &str| -> Vec<(String, String)> {
            v[k].as_array().into_iter().flatten().filter_map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string()))).collect()
        };
        Marks { favorites: pairs("favorites"), recent: pairs("recent"), file }
    }

    fn save(&self) {
        let Some(p) = &self.file else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let v = json!({"favorites": self.favorites, "recent": self.recent});
        let _ = std::fs::write(p, v.to_string());
    }

    pub fn is_favorite(&self, source: &str, key: &str) -> bool {
        self.favorites.iter().any(|(s, k)| s == source && k == key)
    }

    pub fn toggle_favorite(&mut self, source: &str, key: &str) {
        if self.is_favorite(source, key) {
            self.favorites.retain(|(s, k)| !(s == source && k == key));
        } else {
            self.favorites.push((source.to_string(), key.to_string()));
        }
        self.save();
    }

    pub fn favorites<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a str> {
        self.favorites.iter().filter(move |(s, _)| s == source).map(|(_, k)| k.as_str())
    }

    pub fn watched(&mut self, source: &str, key: &str) {
        self.recent.retain(|(s, k)| !(s == source && k == key));
        self.recent.insert(0, (source.to_string(), key.to_string()));
        let mut kept = 0;
        self.recent.retain(|(s, _)| {
            if s != source {
                return true;
            }
            kept += 1;
            kept <= RECENT
        });
        self.save();
    }

    pub fn recent<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a str> {
        self.recent.iter().filter(move |(s, _)| s == source).map(|(_, k)| k.as_str())
    }

    /// Give a source's marks new keys (the ones `rekey` has one for).
    pub fn rekey(&mut self, source: &str, rekey: impl Fn(&str) -> Option<String>) {
        for (s, k) in self.favorites.iter_mut().chain(self.recent.iter_mut()) {
            if s == source
                && let Some(new) = rekey(k)
            {
                *k = new;
            }
        }
        self.save();
    }

    pub fn forget_source(&mut self, source: &str) {
        self.favorites.retain(|(s, _)| s != source);
        self.recent.retain(|(s, _)| s != source);
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iptv::model::Catchup;

    fn channel(kind: CatchupKind, url: &str) -> Channel {
        Channel {
            key: "1".into(),
            name: "One".into(),
            number: None,
            logo: None,
            group: String::new(),
            url: url.into(),
            epg_id: None,
            catchup: Some(Catchup { days: 7, kind }),
            user_agent: None,
            referrer: None,
        }
    }

    #[test]
    fn an_xtream_playlist_link_becomes_a_login() {
        let link = |location: &str| Source {
            id: "a".into(),
            name: "Home".into(),
            kind: Kind::Playlist { location: location.into(), epg: None },
            shelves: BTreeMap::from([("VOD".into(), Shelf::Movies)]),
        };
        let (source, login) = link("http://line.tv:8080/get.php?username=me&password=pw&type=m3u_plus").as_xtream().unwrap();
        assert_eq!(source.kind, Kind::Xtream { server: "http://line.tv:8080".into(), user: "me".into() });
        assert_eq!((source.id.as_str(), source.name.as_str(), source.shelves.len()), ("a", "Home", 0));
        assert_eq!(login.password, "pw");
        assert_eq!(link("http://cdn.tv/list.m3u").as_xtream(), None);
    }

    #[test]
    fn marks_follow_new_keys() {
        let mut m = Marks::default();
        m.toggle_favorite("a", "http://x/me/pw/7");
        m.watched("a", "http://x/me/pw/7");
        m.watched("b", "http://x/me/pw/7");
        m.rekey("a", xtream::stream_id);
        assert_eq!(m.favorites("a").collect::<Vec<_>>(), ["7"]);
        assert_eq!(m.recent("a").collect::<Vec<_>>(), ["7"]);
        assert_eq!(m.recent("b").collect::<Vec<_>>(), ["http://x/me/pw/7"]);
    }

    #[test]
    fn sources_round_trip() {
        for s in [
            Source {
                id: "a".into(),
                name: "Home".into(),
                kind: Kind::Playlist { location: "http://x/list.m3u".into(), epg: None },
                shelves: BTreeMap::from([("VOD".into(), Shelf::Movies)]),
            },
            Source {
                id: "b".into(),
                name: "XC".into(),
                kind: Kind::Xtream { server: "http://x:8080".into(), user: "me".into() },
                shelves: BTreeMap::new(),
            },
        ] {
            assert_eq!(Source::from_json(&s.to_json()), Some(s));
        }
        let s = Source {
            id: "a".into(),
            name: "Home".into(),
            kind: Kind::Playlist { location: "https://cdn.example.com/a/b.m3u?x".into(), epg: None },
            shelves: BTreeMap::new(),
        };
        assert_eq!(s.detail(), "m3u · cdn.example.com");
    }

    #[test]
    fn fills_catchup_templates() {
        let start = 1_793_304_000; // 2026-10-29 20:00 UTC
        let t = fill_template(
            "http://cdn/{Y}/{m}/{d}/{H}-{M}?s={utc}&e={utcend}&d={duration:60}&o={offset:60}",
            start,
            start + 7200,
            start + 3600,
        );
        assert_eq!(t, "http://cdn/2026/10/29/20-00?s=1793304000&e=1793311200&d=120&o=60");
    }

    #[test]
    fn builds_catchup_urls() {
        let now = time::now();
        let p = Programme { start: now - 3600, stop: now - 600, title: String::new(), desc: String::new() };
        let shift = catchup_url(&channel(CatchupKind::Shift, "http://x/live.ts?token=1"), &p, None, 0).unwrap();
        assert!(shift.starts_with(&format!("http://x/live.ts?token=1&utc={}&lutc=", now - 3600)), "{shift}");
        let fl = catchup_url(&channel(CatchupKind::Flussonic, "http://x/ch1/index.m3u8"), &p, None, 0).unwrap();
        assert_eq!(fl, format!("http://x/ch1/index-{}-3000.m3u8", now - 3600));
        let old = Programme { start: now - 30 * 86_400, stop: now - 30 * 86_400 + 60, title: String::new(), desc: String::new() };
        assert!(catchup_url(&channel(CatchupKind::Shift, "http://x"), &old, None, 0).is_none(), "beyond the archive");
        let future = Programme { start: now + 60, stop: now + 120, title: String::new(), desc: String::new() };
        assert!(catchup_url(&channel(CatchupKind::Shift, "http://x"), &future, None, 0).is_none());
    }

    #[test]
    fn keeps_recent_per_source() {
        let mut m = Marks::default();
        for i in 0..40 {
            m.watched("a", &i.to_string());
        }
        m.watched("b", "x");
        m.watched("a", "5");
        assert_eq!(m.recent("a").count(), RECENT);
        assert_eq!(m.recent("a").next(), Some("5"));
        assert_eq!(m.recent("b").collect::<Vec<_>>(), ["x"]);
        m.toggle_favorite("a", "1");
        assert!(m.is_favorite("a", "1") && !m.is_favorite("b", "1"));
        m.toggle_favorite("a", "1");
        assert!(!m.is_favorite("a", "1"));
    }
}
