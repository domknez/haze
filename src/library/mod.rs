//! The viewer's sources (saved in `sources.json`, passwords in the keychain) and
//! loading what each offers. Next to them: [`catchup`] URLs for programmes that
//! already aired, [`Marks`] (favourite and recently watched channels) and
//! [`Progress`] (how far movies and episodes were watched).

mod catchup;
mod marks;
mod progress;

pub use catchup::catchup_url;
pub use marks::Marks;
pub use progress::Progress;

use crate::credentials;
use crate::iptv::model::{Catalog, Channel, Guide, Shelf};
use crate::iptv::{http, m3u, time, xmltv, xtream};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// A file in the settings folder.
fn settings_file(name: &str) -> Option<PathBuf> {
    crate::settings::dir().map(|d| d.join(name))
}

/// The JSON in `file`; null when there's none. One that doesn't parse is kept aside
/// as `<name>.unreadable` (and logged) before anything is saved over it.
fn read_json(file: Option<&Path>) -> Value {
    let Some(p) = file else { return Value::Null };
    let Ok(text) = std::fs::read_to_string(p) else { return Value::Null };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        let mut aside = p.as_os_str().to_owned();
        aside.push(".unreadable");
        log_warn!("{} is unreadable ({e}); kept as {}", p.display(), PathBuf::from(&aside).display());
        let _ = std::fs::rename(p, aside);
        Value::Null
    })
}

/// Write `value` to `file` (see [`crate::settings::save_file`]).
fn write_json(file: Option<&Path>, value: &Value) {
    let Some(p) = file else { return };
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    if let Err(e) = crate::settings::save_file(p, text.as_bytes()) {
        log_warn!("could not save {}: {e}", p.display());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Playlist groups the viewer put elsewhere (channels, movies, series).
    pub shelves: BTreeMap<String, Shelf>,
    /// Groups the viewer hid.
    pub hidden: BTreeSet<String>,
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
            Kind::Playlist { location, .. } if http::is_web(location) => format!("m3u · {}", host(location)),
            Kind::Playlist { location, .. } => format!("m3u · {}", location.rsplit('/').next().unwrap_or(location)),
            Kind::Xtream { server, user } => format!("xtream · {user}@{}", host(server)),
        }
    }

    pub fn login(&self) -> Option<xtream::Login> {
        match &self.kind {
            Kind::Xtream { server, user } => Some(self.xtream_login(server, user)),
            Kind::Playlist { .. } => None,
        }
    }

    /// An Xtream login with the password from the keychain.
    fn xtream_login(&self, server: &str, user: &str) -> xtream::Login {
        xtream::Login::new(server, user, &credentials::load(&self.id).unwrap_or_default())
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
        if !self.hidden.is_empty() {
            v["hidden"] = self.hidden.iter().cloned().collect::<Vec<_>>().into();
        }
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
        let hidden = v["hidden"].as_array().into_iter().flatten().filter_map(|g| Some(g.as_str()?.to_string())).collect();
        Some(Source { id: s("id")?, name: s("name").unwrap_or_else(|| "Source".into()), kind, shelves, hidden })
    }
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or(rest)
}

pub fn new_id() -> String {
    format!("{:08x}", fastrand::u32(..))
}

const SOURCES: &str = "sources.json";

pub fn load_sources() -> Vec<Source> {
    read_json(settings_file(SOURCES).as_deref()).as_array().into_iter().flatten().filter_map(Source::from_json).collect()
}

pub fn save_sources(sources: &[Source]) {
    write_json(settings_file(SOURCES).as_deref(), &sources.iter().map(Source::to_json).collect());
}

/// Everything a source offers.
pub fn fetch(source: &Source) -> Result<Catalog, String> {
    let started = std::time::Instant::now();
    let catalog = match &source.kind {
        Kind::Playlist { location, epg } => read_playlist(location, epg.as_deref(), &source.shelves)?,
        Kind::Xtream { server, user } => source.xtream_login(server, user).catalog().map_err(|e| e.to_string())?,
    };
    log_catalog(&source.name, &catalog, started);
    Ok(catalog)
}

/// The playlist at `location` (a link or a file), sorted onto `shelves`, with the
/// guide `epg` in place of the one it names.
fn read_playlist(location: &str, epg: Option<&str>, shelves: &BTreeMap<String, Shelf>) -> Result<Catalog, String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut http::open(location).map_err(|e| e.to_string())?, &mut text).map_err(|e| e.to_string())?;
    if !m3u::looks_like_playlist(&text) {
        return Err("that address gives a web page or other text, not a playlist".into());
    }
    let mut catalog = m3u::parse_with(&text, shelves);
    if catalog.channels.is_empty() && catalog.movies.is_empty() && catalog.series.is_empty() {
        return Err("the playlist has no channels".into());
    }
    if let Some(epg) = epg {
        catalog.epg_url = Some(epg.to_string());
    }
    Ok(catalog)
}

/// A source that answered, ready to keep: what it offers, and the password to store
/// with it (an Xtream login's, or one that came out of its playlist link).
pub struct Checked {
    pub source: Source,
    pub catalog: Catalog,
    pub secret: Option<String>,
}

/// Load what `source` offers, signing in with `password` when it's an Xtream login.
/// Nothing is stored: the caller keeps the source (and its secret) only once it's
/// wanted.
pub fn check(source: Source, password: String) -> Result<Checked, String> {
    match &source.kind {
        Kind::Xtream { server, user } => {
            let catalog = xtream::Login::new(server, user, &password).catalog().map_err(|e| e.to_string())?;
            Ok(Checked { source, catalog, secret: Some(password) })
        }
        Kind::Playlist { .. } => Ok(match fetch_upgrading(&source)? {
            (catalog, Some((login, secret))) => Checked { source: login, catalog, secret: Some(secret) },
            (catalog, None) => Checked { source, catalog, secret: None },
        }),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_file_is_kept_aside_not_lost() {
        let dir = std::env::temp_dir().join(format!("haze-test-{}", new_id()));
        let file = dir.join("sources.json");
        write_json(Some(&file), &json!([{"id": "a"}]));
        assert_eq!(read_json(Some(&file)), json!([{"id": "a"}]));
        std::fs::write(&file, "[{\"id\": ").unwrap();
        assert_eq!(read_json(Some(&file)), Value::Null);
        assert_eq!(std::fs::read_to_string(dir.join("sources.json.unreadable")).unwrap(), "[{\"id\": ");
        assert!(!file.exists() && !dir.join("sources.json.part").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_xtream_playlist_link_becomes_a_login() {
        let link = |location: &str| Source {
            id: "a".into(),
            name: "Home".into(),
            kind: Kind::Playlist { location: location.into(), epg: None },
            shelves: BTreeMap::from([("VOD".into(), Shelf::Movies)]),
            hidden: BTreeSet::new(),
        };
        let (source, login) = link("http://line.tv:8080/get.php?username=me&password=pw&type=m3u_plus").as_xtream().unwrap();
        assert_eq!(source.kind, Kind::Xtream { server: "http://line.tv:8080".into(), user: "me".into() });
        assert_eq!((source.id.as_str(), source.name.as_str(), source.shelves.len()), ("a", "Home", 0));
        assert_eq!(login.password, "pw");
        assert_eq!(link("http://cdn.tv/list.m3u").as_xtream(), None);
    }

    #[test]
    fn sources_round_trip() {
        for s in [
            Source {
                id: "a".into(),
                name: "Home".into(),
                kind: Kind::Playlist { location: "http://x/list.m3u".into(), epg: None },
                shelves: BTreeMap::from([("VOD".into(), Shelf::Movies)]),
                hidden: BTreeSet::from(["Adult".into()]),
            },
            Source {
                id: "b".into(),
                name: "XC".into(),
                kind: Kind::Xtream { server: "http://x:8080".into(), user: "me".into() },
                shelves: BTreeMap::new(),
                hidden: BTreeSet::new(),
            },
        ] {
            assert_eq!(Source::from_json(&s.to_json()), Some(s));
        }
        let s = Source {
            id: "a".into(),
            name: "Home".into(),
            kind: Kind::Playlist { location: "https://cdn.example.com/a/b.m3u?x".into(), epg: None },
            shelves: BTreeMap::new(),
            hidden: BTreeSet::new(),
        };
        assert_eq!(s.detail(), "m3u · cdn.example.com");
    }
}
