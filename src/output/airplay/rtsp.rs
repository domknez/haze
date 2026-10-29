//! Minimal RTSP client for AirPlay control, encrypted once AirPlay 2 pairing is done.

use super::ap2::pairing::{DecryptReader, Encryptor, hkdf32};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

pub struct Rtsp {
    writer: TcpStream,
    /// Set once AirPlay 2 pairing completes; everything after is encrypted.
    encryptor: Option<Encryptor>,
    reader: BufReader<DecryptReader<TcpStream>>,
    user_agent: &'static str,
    cseq: u32,
    /// Session URL: `rtsp://<our ip>/<session id>`.
    url: String,
    session: Option<String>,
    /// DACP id (shared with the remote-control server) and per-session remote id.
    dacp_id: String,
    active_remote: String,
}

pub struct Response {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

impl Rtsp {
    pub fn new(stream: TcpStream, url: String, user_agent: &'static str, dacp_id: String) -> Result<Self, String> {
        Ok(Self {
            reader: BufReader::new(DecryptReader::new(stream.try_clone().map_err(|e| e.to_string())?)),
            writer: stream,
            encryptor: None,
            user_agent,
            cseq: 0,
            url,
            session: None,
            dacp_id,
            active_remote: fastrand::u32(..).to_string(),
        })
    }

    /// Switch both directions to the ChaCha20-Poly1305 control channel.
    pub fn encrypt(&mut self, session_key: &[u8]) {
        self.encryptor = Some(Encryptor::new(&hkdf32(session_key, "Control-Salt", "Control-Write-Encryption-Key")));
        self.reader.get_mut().enable(&hkdf32(session_key, "Control-Salt", "Control-Read-Encryption-Key"));
    }

    /// Another handle to the connection (e.g. to shut it down from another thread).
    pub fn socket(&self) -> std::io::Result<TcpStream> {
        self.writer.try_clone()
    }

    /// How long to wait for each reply (the connection's read timeout).
    pub fn set_reply_timeout(&self, timeout: std::time::Duration) {
        let _ = self.writer.set_read_timeout(Some(timeout));
    }

    /// Remember the Session header of a SETUP reply for later requests.
    pub fn adopt_session(&mut self, resp: &Response) {
        self.session = resp.header("Session").map(|s| s.split(';').next().unwrap_or(s).to_string());
    }

    /// Send a request (to the session URL unless `uri` is given) and read the reply.
    /// Non-200 statuses become errors.
    pub fn request(
        &mut self,
        method: &str,
        uri: Option<&str>,
        headers: &[(&str, String)],
        body: Option<(&str, &[u8])>,
    ) -> Result<Response, String> {
        self.cseq += 1;
        let mut req = format!(
            "{method} {} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: {}\r\nClient-Instance: {}\r\nDACP-ID: {}\r\nActive-Remote: {}\r\n",
            uri.unwrap_or(&self.url),
            self.cseq,
            self.user_agent,
            self.dacp_id,
            self.dacp_id,
            self.active_remote
        );
        if let Some(s) = &self.session {
            req += &format!("Session: {s}\r\n");
        }
        for (k, v) in headers {
            req += &format!("{k}: {v}\r\n");
        }
        if let Some((content_type, b)) = body {
            req += &format!("Content-Type: {content_type}\r\nContent-Length: {}\r\n", b.len());
        }
        req += "\r\n";
        let mut bytes = req.into_bytes();
        if let Some((_, b)) = body {
            bytes.extend_from_slice(b);
        }
        if let Some(e) = self.encryptor.as_mut() {
            bytes = e.seal(&bytes);
        }
        self.writer.write_all(&bytes).map_err(|e| format!("{method}: {e}"))?;
        let (status, resp) = self.read_response().map_err(|e| format!("{method}: {e}"))?;
        match status {
            200 => Ok(resp),
            401 => Err("AirPlay device requires a password (not supported)".into()),
            470 => Err("AirPlay device requires PIN pairing (not supported)".into()),
            s => Err(format!("{method} failed: RTSP {s}")),
        }
    }

    fn read_response(&mut self) -> Result<(u16, Response), String> {
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| format!("bad response {line:?}"))?;
        let mut headers = Vec::new();
        loop {
            line.clear();
            self.reader.read_line(&mut line).map_err(|e| e.to_string())?;
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        let mut resp = Response { headers, body: Vec::new() };
        if let Some(len) = resp.header("Content-Length").and_then(|l| l.parse::<usize>().ok()) {
            resp.body = vec![0; len];
            self.reader.read_exact(&mut resp.body).map_err(|e| e.to_string())?;
        }
        Ok((status, resp))
    }
}

/// A numeric parameter of an RTSP `Transport` header, e.g. `server_port`.
pub fn transport_param(t: &str, key: &str) -> Option<u16> {
    t.split(';').find_map(|p| p.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_parse() {
        let t = "RTP/AVP/UDP;unicast;mode=record;server_port=53561;control_port=63379;timing_port=50607";
        assert_eq!(transport_param(t, "server_port"), Some(53561));
        assert_eq!(transport_param(t, "control_port"), Some(63379));
        assert_eq!(transport_param(t, "missing"), None);
    }
}
