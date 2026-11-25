//! Where to open a stream from, worked out before ffmpeg opens it.
//!
//! - HLS: a master playlist lists the same channel in several qualities, and ffmpeg
//!   would download and probe every one of them before playing. When each quality
//!   carries its own sound, the best one is opened directly.
//! - DASH: ffmpeg finds a manifest's segments from the address it was given, not
//!   from the one a redirect led to, so they would not be found. A manifest is
//!   opened from where it really is.

use std::time::Duration;
use ureq::ResponseExt;

/// Where to open the stream at `url` from: the best quality of an HLS master
/// playlist, a DASH manifest's address after redirects, otherwise `url` itself.
pub fn location(url: &str, user_agent: &str, referrer: Option<&str>) -> String {
    let found = if names_file(url, ".m3u8") {
        best_quality(url, user_agent, referrer)
    } else if names_file(url, ".mpd") {
        fetch(url, user_agent, referrer).map(|resp| resp.get_uri().to_string())
    } else {
        None
    };
    found.unwrap_or_else(|| url.to_string())
}

/// The media playlist of the best quality, if `url` is a master playlist whose
/// qualities are self-contained.
fn best_quality(url: &str, user_agent: &str, referrer: Option<&str>) -> Option<String> {
    let mut resp = fetch(url, user_agent, referrer)?;
    // Redirects: relative URIs resolve against where the playlist really is.
    let base = resp.get_uri().to_string();
    let text = resp.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    let chosen = best_variant(&text)?;
    Some(resolve(&base, &chosen))
}

/// The answer to a GET of `url`, after redirects.
fn fetch(url: &str, user_agent: &str, referrer: Option<&str>) -> Option<ureq::http::Response<ureq::Body>> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).user_agent(user_agent).build().into();
    let mut req = agent.get(url);
    if let Some(r) = referrer {
        req = req.header("Referer", r);
    }
    req.call().ok()
}

/// Whether `url` is a web address of a file with the extension `ext` (lowercase).
fn names_file(url: &str, ext: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    crate::iptv::http::is_web(&lower) && lower.split('?').next().is_some_and(|path| path.ends_with(ext))
}

/// The URI of the highest-bandwidth variant, unless the playlist keeps sound (or
/// anything else) in separate renditions.
fn best_variant(text: &str) -> Option<String> {
    if !text.contains("#EXT-X-STREAM-INF") || text.contains("#EXT-X-MEDIA:") {
        return None;
    }
    let mut best: Option<(u64, String)> = None;
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") else { continue };
        let bandwidth = attrs.split(',').find_map(|a| a.trim().strip_prefix("BANDWIDTH=")).and_then(|b| b.parse::<u64>().ok()).unwrap_or(0);
        let Some(uri) = lines.by_ref().find(|l| !l.is_empty() && !l.starts_with('#')) else { break };
        if best.as_ref().is_none_or(|(b, _)| bandwidth > *b) {
            best = Some((bandwidth, uri.to_string()));
        }
    }
    best.map(|(_, uri)| uri)
}

fn resolve(base: &str, uri: &str) -> String {
    if uri.contains("://") {
        return uri.to_string();
    }
    let base = base.split('?').next().unwrap_or(base);
    if let Some(path) = uri.strip_prefix('/') {
        let origin_end = base.find("://").map_or(0, |i| i + 3);
        let host_end = base[origin_end..].find('/').map_or(base.len(), |i| origin_end + i);
        return format!("{}/{path}", &base[..host_end]);
    }
    match base.rfind('/') {
        Some(i) => format!("{}/{uri}", &base[..i]),
        None => uri.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow/index.m3u8\n\
                          #EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH=5000000,RESOLUTION=1920x1080\nhigh/index.m3u8?token=1\n\
                          #EXT-X-STREAM-INF:BANDWIDTH=2000000\nmid/index.m3u8\n";

    #[test]
    fn picks_the_best_self_contained_variant() {
        assert_eq!(best_variant(MASTER).as_deref(), Some("high/index.m3u8?token=1"));
        assert_eq!(best_variant("#EXTM3U\n#EXTINF:6,\nseg1.ts\n"), None, "already a media playlist");
        let separate_audio = format!("{MASTER}#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aac\",URI=\"a.m3u8\"\n");
        assert_eq!(best_variant(&separate_audio), None);
    }

    #[test]
    fn knows_playlists_and_manifests() {
        assert!(names_file("https://h/tv/ch1/index.mpd", ".mpd"));
        assert!(names_file("https://h/tv/ch1/INDEX.MPD?t=1", ".mpd"));
        assert!(names_file("http://h/live/master.m3u8?t=1", ".m3u8"));
        assert!(!names_file("https://h/tv/ch1/index.m3u8", ".mpd"));
        assert!(!names_file("/local/file.mpd", ".mpd"));
    }

    #[test]
    fn resolves_relative_uris() {
        assert_eq!(resolve("http://h/live/ch1/master.m3u8?t=1", "high/i.m3u8"), "http://h/live/ch1/high/i.m3u8");
        assert_eq!(resolve("http://h/live/ch1/master.m3u8", "/abs/i.m3u8"), "http://h/abs/i.m3u8");
        assert_eq!(resolve("http://h/a.m3u8", "https://cdn/x.m3u8"), "https://cdn/x.m3u8");
    }
}
