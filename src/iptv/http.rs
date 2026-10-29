//! Fetching playlists, guides and provider API answers.

use std::io::{BufRead, BufReader, Read};
use std::sync::LazyLock;
use std::time::Duration;

/// Providers often reject requests without a player-like user agent.
pub const USER_AGENT: &str = concat!("haze/", env!("CARGO_PKG_VERSION"), " (like VLC/3.0)");

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        // Big playlists and guides take a while on slow providers.
        .timeout_global(Some(Duration::from_secs(180)))
        .user_agent(USER_AGENT)
        .build()
        .into()
});

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Http(#[from] ureq::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the server's answer is not what was expected: {0}")]
    Unexpected(String),
}

pub fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, Error> {
    Ok(AGENT.get(url).call()?)
}

pub fn text(url: &str) -> Result<String, Error> {
    let mut s = String::new();
    reader(url)?.read_to_string(&mut s)?;
    Ok(s)
}

pub fn json(url: &str) -> Result<serde_json::Value, Error> {
    let text = text(url)?;
    serde_json::from_str(&text).map_err(|e| Error::Unexpected(e.to_string()))
}

/// The response body, gunzipped if it is gzip (guides usually are, whatever the
/// headers say).
pub fn reader(url: &str) -> Result<Box<dyn BufRead + Send>, Error> {
    let body = get(url)?.into_body().into_reader();
    Ok(maybe_gunzip(BufReader::with_capacity(64 << 10, body))?)
}

/// A file or URL as a reader, gunzipped if needed.
pub fn open(location: &str) -> Result<Box<dyn BufRead + Send>, Error> {
    if location.starts_with("http://") || location.starts_with("https://") {
        reader(location)
    } else {
        let file = std::fs::File::open(location.strip_prefix("file://").unwrap_or(location))?;
        Ok(maybe_gunzip(BufReader::with_capacity(64 << 10, file))?)
    }
}

fn maybe_gunzip<R: BufRead + Send + 'static>(mut r: R) -> std::io::Result<Box<dyn BufRead + Send>> {
    let gzip = r.fill_buf()?.starts_with(&[0x1f, 0x8b]);
    Ok(if gzip { Box::new(BufReader::new(flate2::bufread::MultiGzDecoder::new(r))) } else { Box::new(r) })
}

/// `s` with characters that are special in a URL query escaped.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `%XX` escapes and `+` read back, as in a URL's query.
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn gunzips_only_gzip() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(b"<tv/>").unwrap();
        let bytes = gz.finish().unwrap();
        let mut s = String::new();
        maybe_gunzip(std::io::Cursor::new(bytes)).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "<tv/>");
        s.clear();
        maybe_gunzip(std::io::Cursor::new(b"<tv/>".to_vec())).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "<tv/>");
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
