//! Xtream Codes providers: a server address, user name and password instead of a
//! playlist link. The `player_api.php` endpoint lists categories, channels and the
//! video library; streams, the guide and the archive have URLs of their own.

use super::http::{self, Error, decode, encode};
use super::model::{Account, Catalog, Catchup, CatchupKind, Channel, Episode, Movie, Series};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Login {
    /// `http://host:port`, without a trailing slash.
    pub server: String,
    pub user: String,
    pub password: String,
}

impl Login {
    pub fn new(server: &str, user: &str, password: &str) -> Login {
        let mut server = server.trim().trim_end_matches('/').to_string();
        if !server.contains("://") {
            server = format!("http://{server}");
        }
        // People paste the playlist link or the API URL: keep only the server.
        for tail in ["/player_api.php", "/get.php"] {
            if let Some(i) = server.find(tail) {
                server.truncate(i);
            }
        }
        Login { server, user: user.trim().to_string(), password: password.to_string() }
    }

    /// The login in an Xtream panel's playlist link
    /// (`http://server/get.php?username=…&password=…&type=m3u_plus`).
    pub fn from_playlist_link(link: &str) -> Option<Login> {
        let (path, query) = link.trim().split_once('?')?;
        if !path.ends_with("/get.php") {
            return None;
        }
        let param = |name: &str| {
            query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| decode(v)).filter(|v| !v.is_empty())
        };
        Some(Login::new(path, &param("username")?, &param("password")?))
    }

    fn api(&self, action: Option<&str>) -> String {
        let mut url = format!("{}/player_api.php?username={}&password={}", self.server, encode(&self.user), encode(&self.password));
        if let Some(a) = action {
            url.push_str("&action=");
            url.push_str(a);
        }
        url
    }

    fn path(&self, kind: &str) -> String {
        format!("{}/{kind}/{}/{}", self.server, encode(&self.user), encode(&self.password))
    }

    pub fn live_url(&self, stream_id: u64) -> String {
        format!("{}/{stream_id}.ts", self.path("live"))
    }

    pub fn movie_url(&self, stream_id: u64, extension: &str) -> String {
        format!("{}/{stream_id}.{extension}", self.path("movie"))
    }

    pub fn episode_url(&self, episode_id: u64, extension: &str) -> String {
        format!("{}/{episode_id}.{extension}", self.path("series"))
    }

    pub fn guide_url(&self) -> String {
        format!("{}/xmltv.php?username={}&password={}", self.server, encode(&self.user), encode(&self.password))
    }

    /// The archived stream from `start` (Unix seconds) for `minutes`. Xtream servers
    /// want the start on their own wall clock, `server_offset` seconds east of UTC.
    pub fn timeshift_url(&self, stream_id: u64, start: i64, minutes: u32, server_offset: i64) -> String {
        let t = super::time::civil(start + server_offset);
        format!(
            "{}/{minutes}/{:04}-{:02}-{:02}:{:02}-{:02}/{stream_id}.ts",
            self.path("timeshift"),
            t.year,
            t.month,
            t.day,
            t.hour,
            t.minute
        )
    }

    /// Checks the login; the account's status if the provider accepts it.
    pub fn account(&self) -> Result<Account, Error> {
        let v = http::json(&self.api(None))?;
        let info = v.get("user_info").ok_or_else(|| Error::Unexpected("no user_info".into()))?;
        if number(&info["auth"]) == Some(0) {
            return Err(Error::Unexpected("the provider did not accept the user name or password".into()));
        }
        // The server's clock as text and as a timestamp tell its time zone.
        let server = &v["server_info"];
        let server_offset = match (string(&server["time_now"]).and_then(|t| super::time::parse_plain(&t)), number(&server["timestamp_now"]))
        {
            // Rounded to quarter hours: the two are read a moment apart.
            (Some(wall), Some(utc)) => ((wall - utc as i64) as f64 / 900.0).round() as i64 * 900,
            _ => 0,
        };
        Ok(Account {
            server_offset,
            status: string(&info["status"]).unwrap_or_else(|| "Active".into()),
            expires: number(&info["exp_date"]).map(|n| n as i64).filter(|&n| n > 0),
            max_connections: number(&info["max_connections"]).map(|n| n as u32),
            active_connections: number(&info["active_cons"]).map(|n| n as u32),
        })
    }

    /// The account, live channels, movies and series.
    pub fn catalog(&self) -> Result<Catalog, Error> {
        let account = self.account()?;
        let live_groups = categories(&http::json(&self.api(Some("get_live_categories")))?);
        let channels: Vec<Channel> =
            list(&http::json(&self.api(Some("get_live_streams")))?).filter_map(|v| self.channel(v, &live_groups)).collect();
        let movies = self.library("vod", |v, groups| self.movie(v, groups));
        let series = self.library("series", series);
        let groups = order_groups(&channels, &live_groups);
        Ok(Catalog {
            channels,
            groups,
            movie_groups: Catalog::groups_of_movies(&movies),
            movies,
            series_groups: Catalog::groups_of_series(&series),
            series,
            epg_url: Some(self.guide_url()),
            account: Some(account),
        })
    }

    /// One part of the video library (`vod` or `series`). It's a bonus: a provider
    /// without one, or failing to list it, still gives a working catalog.
    fn library<T>(&self, kind: &str, read: impl Fn(&Value, &[(String, String)]) -> Option<T>) -> Vec<T> {
        let streams = if kind == "series" { "get_series".to_string() } else { format!("get_{kind}_streams") };
        match (http::json(&self.api(Some(&format!("get_{kind}_categories")))), http::json(&self.api(Some(&streams)))) {
            (Ok(cats), Ok(items)) => {
                let groups = categories(&cats);
                list(&items).filter_map(|v| read(v, &groups)).collect()
            }
            (Err(e), _) | (_, Err(e)) => {
                log_warn!("xtream: no {kind} library: {e}");
                Vec::new()
            }
        }
    }

    /// The episodes of a series (its key is the provider's series id).
    pub fn episodes(&self, series_key: &str) -> Result<Vec<Episode>, Error> {
        let v = http::json(&format!("{}&series_id={}", self.api(Some("get_series_info")), encode(series_key)))?;
        // By season (`{"1": [...]}`), or a list of seasons' lists on some panels.
        let seasons: Vec<&Value> = match &v["episodes"] {
            Value::Object(m) => m.values().collect(),
            Value::Array(a) => a.iter().collect(),
            _ => Vec::new(),
        };
        let mut out: Vec<Episode> = seasons
            .into_iter()
            .flat_map(|s| if s.is_array() { list(s).collect() } else { vec![s] })
            .filter_map(|e| {
                let id = number(&e["id"])?;
                let ext = string(&e["container_extension"]).unwrap_or_else(|| "mp4".into());
                Some(Episode {
                    key: id.to_string(),
                    season: number(&e["season"]).unwrap_or(1) as u32,
                    number: number(&e["episode_num"]).unwrap_or(0) as u32,
                    title: string(&e["title"]).map(|t| t.trim().to_string()).unwrap_or_default(),
                    url: self.episode_url(id, &ext),
                })
            })
            .collect();
        out.sort_by_key(|e| (e.season, e.number));
        Ok(out)
    }

    fn channel(&self, v: &Value, groups: &[(String, String)]) -> Option<Channel> {
        let id = number(&v["stream_id"])?;
        let archive = number(&v["tv_archive"]).unwrap_or(0) > 0;
        Some(Channel {
            key: id.to_string(),
            name: string(&v["name"])?.trim().to_string(),
            number: number(&v["num"]).map(|n| n as u32),
            logo: string(&v["stream_icon"]).filter(|s| s.starts_with("http")),
            group: group_name(&v["category_id"], groups),
            url: self.live_url(id),
            epg_id: string(&v["epg_channel_id"]),
            catchup: archive.then(|| Catchup {
                days: number(&v["tv_archive_duration"]).unwrap_or(1) as u32,
                kind: CatchupKind::Xtream { stream_id: id },
            }),
            user_agent: None,
            referrer: None,
        })
    }

    fn movie(&self, v: &Value, groups: &[(String, String)]) -> Option<Movie> {
        let id = number(&v["stream_id"])?;
        let ext = string(&v["container_extension"]).unwrap_or_else(|| "mp4".into());
        let name = string(&v["name"])?.trim().to_string();
        Some(Movie {
            key: id.to_string(),
            year: string(&v["year"]).and_then(|y| y.parse().ok()).or_else(|| year_in(&name)),
            name,
            poster: string(&v["stream_icon"]).filter(|s| s.starts_with("http")),
            group: group_name(&v["category_id"], groups),
            url: self.movie_url(id, &ext),
            rating: string(&v["rating"])
                .and_then(|r| r.parse::<f32>().ok())
                .or_else(|| v["rating"].as_f64().map(|f| f as f32))
                .filter(|r| *r > 0.0),
        })
    }
}

fn series(v: &Value, groups: &[(String, String)]) -> Option<Series> {
    let name = string(&v["name"])?.trim().to_string();
    let year = ["year", "releaseDate", "release_date"]
        .iter()
        .find_map(|k| string(&v[*k]).and_then(|d| d.get(..4)?.parse().ok()))
        .or_else(|| year_in(&name));
    Some(Series {
        key: number(&v["series_id"])?.to_string(),
        year,
        name,
        poster: string(&v["cover"]).filter(|s| s.starts_with("http")),
        group: group_name(&v["category_id"], groups),
        rating: string(&v["rating"]).and_then(|r| r.parse::<f32>().ok()).filter(|r| *r > 0.0),
        episodes: Vec::new(),
    })
}

/// Groups in the provider's category order, keeping only those with channels.
fn order_groups(channels: &[Channel], categories: &[(String, String)]) -> Vec<String> {
    let present = Catalog::groups_of(channels);
    let mut out: Vec<String> = categories.iter().map(|(_, n)| n.clone()).filter(|n| present.contains(n)).collect();
    out.extend(present.into_iter().filter(|g| !out.contains(g)).collect::<Vec<_>>());
    out
}

/// `(id, name)` of each category.
fn categories(v: &Value) -> Vec<(String, String)> {
    list(v).filter_map(|c| Some((string(&c["category_id"])?, string(&c["category_name"])?.trim().to_string()))).collect()
}

fn group_name(id: &Value, groups: &[(String, String)]) -> String {
    let id = string(id).unwrap_or_default();
    groups.iter().find(|(i, _)| *i == id).map_or_else(|| super::m3u::NO_GROUP.to_string(), |(_, n)| n.clone())
}

fn list(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

/// Xtream servers send numbers as numbers or as strings, depending on the panel.
fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `Dune (2021)` → 2021.
pub fn year_in(name: &str) -> Option<u32> {
    let open = name.rfind('(')?;
    let y: u32 = name[open + 1..].strip_suffix(')').or_else(|| name[open + 1..].get(..4))?.parse().ok()?;
    (1900..2100).contains(&y).then_some(y)
}

/// The stream id at the end of an Xtream stream link (`…/user/pass/123.ts` → `123`),
/// which is what a channel's key is when its source signs in.
pub fn stream_id(url: &str) -> Option<String> {
    let last = url.split('?').next()?.rsplit('/').next()?;
    let id = last.split('.').next()?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn login() -> Login {
        Login::new("line.example.tv:8080/", "me", "p@ss word")
    }

    #[test]
    fn cleans_up_the_server_address() {
        assert_eq!(login().server, "http://line.example.tv:8080");
        assert_eq!(Login::new("https://x.tv/get.php?username=a&password=b&type=m3u", "a", "b").server, "https://x.tv");
        assert_eq!(Login::new("http://x.tv/player_api.php", "a", "b").server, "http://x.tv");
    }

    #[test]
    fn reads_the_login_in_a_playlist_link() {
        let l = Login::from_playlist_link("http://line.tv:8080/get.php?username=me&password=p%40ss+w&type=m3u_plus&output=ts").unwrap();
        assert_eq!((l.server.as_str(), l.user.as_str(), l.password.as_str()), ("http://line.tv:8080", "me", "p@ss w"));
        assert_eq!(Login::from_playlist_link("http://cdn.tv/list.m3u?username=me&password=x"), None);
        assert_eq!(Login::from_playlist_link("http://line.tv/get.php?username=me"), None);
    }

    #[test]
    fn stream_ids_from_links() {
        assert_eq!(stream_id("http://line.tv:8080/me/pw/123").as_deref(), Some("123"));
        assert_eq!(stream_id("http://line.tv/live/me/pw/45.m3u8").as_deref(), Some("45"));
        assert_eq!(stream_id("http://cdn.tv/hrt1.ts"), None);
    }

    #[test]
    fn builds_urls() {
        let l = login();
        assert_eq!(
            l.api(Some("get_live_streams")),
            "http://line.example.tv:8080/player_api.php?username=me&password=p%40ss%20word&action=get_live_streams"
        );
        assert_eq!(l.live_url(42), "http://line.example.tv:8080/live/me/p%40ss%20word/42.ts");
        assert_eq!(l.movie_url(7, "mkv"), "http://line.example.tv:8080/movie/me/p%40ss%20word/7.mkv");
        // 2026-10-29 20:00 UTC
        assert_eq!(
            l.timeshift_url(42, 1_793_304_000, 120, 0),
            "http://line.example.tv:8080/timeshift/me/p%40ss%20word/120/2026-10-29:20-00/42.ts"
        );
        assert!(l.timeshift_url(42, 1_793_304_000, 60, 3600).contains("/2026-10-29:21-00/"), "on the server's clock");
    }

    #[test]
    fn reads_channels_whatever_the_number_format() {
        let groups = categories(&json!([{"category_id": "5", "category_name": "Sports "}, {"category_id": 6, "category_name": "News"}]));
        let l = login();
        let c = l
            .channel(
                &json!({"num": "101", "name": "Arena Sport 1", "stream_id": 42, "stream_icon": "http://logo", "epg_channel_id": "arena1.hr",
                        "category_id": "5", "tv_archive": "1", "tv_archive_duration": "7"}),
                &groups,
            )
            .unwrap();
        assert_eq!((c.key.as_str(), c.number, c.group.as_str()), ("42", Some(101), "Sports"));
        assert_eq!(c.catchup, Some(Catchup { days: 7, kind: CatchupKind::Xtream { stream_id: 42 } }));
        let c =
            l.channel(&json!({"name": "CNN", "stream_id": "9", "stream_icon": "", "category_id": 6, "tv_archive": 0}), &groups).unwrap();
        assert_eq!((c.group.as_str(), c.logo.clone(), c.catchup.clone()), ("News", None, None));
        assert!(l.channel(&json!({"name": "no id"}), &groups).is_none());
    }

    #[test]
    fn reads_movies() {
        let m = login()
            .movie(&json!({"name": "Dune (2021)", "stream_id": 7, "container_extension": "mkv", "rating": "7.9", "category_id": "1"}), &[])
            .unwrap();
        assert_eq!((m.year, m.rating, m.group.as_str()), (Some(2021), Some(7.9), super::super::m3u::NO_GROUP));
    }

    /// A provider on localhost answering the API like an Xtream Codes panel.
    fn fake_provider(auth: u8) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                let mut header = String::new();
                while reader.read_line(&mut header).unwrap_or(0) > 2 {
                    header.clear();
                }
                let path = line.split_whitespace().nth(1).unwrap_or("");
                let action = path.split("action=").nth(1).unwrap_or("").split('&').next().unwrap_or("");
                let body = match action {
                    "" => json!({"user_info": {"auth": auth, "status": "Active", "exp_date": "1800000000", "max_connections": "2", "active_cons": "0"},
                                 "server_info": {"timestamp_now": 1_793_304_000, "time_now": "2026-10-29 21:00:00"}}),
                    "get_live_categories" => json!([{"category_id": "1", "category_name": "Sports"}, {"category_id": "2", "category_name": "News"}]),
                    "get_live_streams" => json!([
                        {"num": 1, "name": "CNN", "stream_id": 9, "category_id": "2", "epg_channel_id": "cnn.us", "tv_archive": 0},
                        {"num": 2, "name": "Arena Sport 1", "stream_id": "42", "category_id": "1", "tv_archive": 1, "tv_archive_duration": "7"}
                    ]),
                    "get_vod_categories" => json!([{"category_id": "7", "category_name": "Films"}]),
                    "get_vod_streams" => json!([{"name": "Dune (2021)", "stream_id": 5, "category_id": "7", "container_extension": "mkv"}]),
                    "get_series_categories" => json!([{"category_id": "3", "category_name": "Drama"}]),
                    "get_series" => json!([{"name": "Dark", "series_id": 11, "category_id": "3", "cover": "http://img/dark.jpg", "releaseDate": "2017-12-01"}]),
                    "get_series_info" => json!({"episodes": {
                        "2": [{"id": "202", "episode_num": 1, "season": 2, "title": "Beginnings", "container_extension": "mkv"}],
                        "1": [{"id": "102", "episode_num": "2", "season": 1, "title": "Lies"}, {"id": "101", "episode_num": 1, "season": 1, "title": "Secrets"}]
                    }}),
                    _ => json!([]),
                }
                .to_string();
                let mut out = stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("{addr}")
    }

    #[test]
    fn loads_a_whole_catalog() {
        let login = Login::new(&fake_provider(1), "me", "pw");
        let c = login.catalog().unwrap();
        assert_eq!(c.groups, ["Sports", "News"], "the provider's order");
        assert_eq!(c.channels.len(), 2);
        assert_eq!(c.channels[1].url, format!("{}/live/me/pw/42.ts", login.server));
        assert_eq!(c.movies.len(), 1);
        assert_eq!(c.movies[0].url, format!("{}/movie/me/pw/5.mkv", login.server));
        assert_eq!((c.series[0].key.as_str(), c.series[0].group.as_str(), c.series[0].year), ("11", "Drama", Some(2017)));
        let episodes = login.episodes(&c.series[0].key).unwrap();
        let codes: Vec<_> = episodes.iter().map(|e| (e.code(), e.title.as_str())).collect();
        assert_eq!(codes, [("S1 E1".into(), "Secrets"), ("S1 E2".into(), "Lies"), ("S2 E1".into(), "Beginnings")]);
        assert_eq!(episodes[2].url, format!("{}/series/me/pw/202.mkv", login.server));
        assert_eq!(episodes[0].url, format!("{}/series/me/pw/101.mp4", login.server));
        let account = c.account.unwrap();
        assert_eq!((account.expires, account.max_connections, account.server_offset), (Some(1_800_000_000), Some(2), 3600));
        assert_eq!(c.epg_url, Some(login.guide_url()));
    }

    #[test]
    fn says_when_the_login_is_wrong() {
        let err = Login::new(&fake_provider(0), "me", "bad").catalog().unwrap_err().to_string();
        assert!(err.contains("did not accept"), "{err}");
    }

    #[test]
    fn keeps_category_order() {
        let ch = |g: &str| Channel {
            key: g.into(),
            name: g.into(),
            number: None,
            logo: None,
            group: g.into(),
            url: String::new(),
            epg_id: None,
            catchup: None,
            user_agent: None,
            referrer: None,
        };
        let cats = vec![("1".into(), "A".into()), ("2".into(), "B".into()), ("3".into(), "Empty".into())];
        assert_eq!(order_groups(&[ch("Other"), ch("B"), ch("A")], &cats), ["A", "B", "Other"]);
    }
}
