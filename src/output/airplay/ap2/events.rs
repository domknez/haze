//! AirPlay 2 event channel. After SETUP the sender connects to the receiver's
//! `eventPort`; the receiver then sends RTSP-style requests over it (e.g.
//! `POST /command` with a binary plist), encrypted like the control channel but
//! with the "Events" keys. Receivers such as Sonos report their hardware volume
//! and transport buttons here rather than over DACP.

use super::bplist::{self, Value};
use super::pairing::{Encryptor, hkdf32};
use crate::output::airplay::dacp::{self, Remote};
use crate::output::airplay::volume;
use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use std::io::{self, Read, Write};
use std::net::TcpStream;

const SALT: &str = "Events-Salt";
const KEY_INFOS: [&str; 2] = ["Events-Write-Encryption-Key", "Events-Read-Encryption-Key"];

/// Serve the event channel on its own thread until the receiver closes it.
pub fn spawn(stream: TcpStream, session_key: &[u8]) {
    let keys = KEY_INFOS.map(|info| hkdf32(session_key, SALT, info));
    let spawned = std::thread::Builder::new().name("airplay-events".into()).spawn(move || match serve(stream, keys) {
        Ok(()) => log_debug!("airplay events: channel closed"),
        Err(e) => log_debug!("airplay events: {e}"),
    });
    if let Err(e) = spawned {
        log_warn!("airplay events thread: {e}");
    }
}

fn serve(stream: TcpStream, keys: [[u8; 32]; 2]) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut frames = Frames { inner: stream, keys, read: None, counter: 0 };
    let mut reply: Option<Encryptor> = None;
    let mut buf = Vec::new();
    loop {
        // Requests arrive as one or more frames; parse once a full request is buffered.
        while request_len(&buf).is_none() {
            match frames.next()? {
                Some(plain) => buf.extend_from_slice(&plain),
                None => return Ok(()),
            }
        }
        let len = request_len(&buf).expect("checked above");
        let request: Vec<u8> = buf.drain(..len).collect();
        let cseq = handle(&request);
        // Reply with the key the receiver didn't use.
        let encryptor = reply.get_or_insert_with(|| Encryptor::new(&keys[1 - frames.read.unwrap_or(0)]));
        let resp = format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nServer: AirTunes/366.0\r\nContent-Length: 0\r\n\r\n");
        writer.write_all(&encryptor.seal(resp.as_bytes()))?;
    }
}

/// Decrypted frames; the read key is whichever of the two decrypts the first frame.
struct Frames {
    inner: TcpStream,
    keys: [[u8; 32]; 2],
    read: Option<usize>,
    counter: u64,
}

impl Frames {
    fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut len = [0u8; 2];
        match self.inner.read_exact(&mut len) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            r => r?,
        }
        let n = u16::from_le_bytes(len) as usize;
        let mut frame = vec![0u8; n + 16];
        self.inner.read_exact(&mut frame)?;
        let candidates = match self.read {
            Some(i) => vec![i],
            None => vec![0, 1],
        };
        for i in candidates {
            if let Some(plain) = open(&self.keys[i], self.counter, &len, &frame) {
                self.read = Some(i);
                self.counter += 1;
                return Ok(Some(plain));
            }
        }
        Err(io::Error::new(io::ErrorKind::InvalidData, "event channel decrypt failed"))
    }
}

fn open(key: &[u8; 32], counter: u64, aad: &[u8], frame: &[u8]) -> Option<Vec<u8>> {
    let n = frame.len() - 16;
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_le_bytes());
    let tag = Tag::try_from(&frame[n..]).ok()?;
    let mut buf = frame[..n].to_vec();
    let cipher = ChaCha20Poly1305::new(&Key::from(*key));
    cipher.decrypt_inout_detached(&Nonce::from(nonce), aad, buf.as_mut_slice().into(), &tag).ok()?;
    Some(buf)
}

/// Length of the first complete request (head + Content-Length body) in `buf`.
fn request_len(buf: &[u8]) -> Option<usize> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let body = header(&head, "Content-Length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    (buf.len() >= head_end + body).then_some(head_end + body)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
}

/// Act on one request; returns its CSeq for the reply.
fn handle(request: &[u8]) -> String {
    let head_end = request.windows(4).position(|w| w == b"\r\n\r\n").map_or(request.len(), |p| p + 4);
    let head = String::from_utf8_lossy(&request[..head_end]);
    let body = &request[head_end..];
    let line = head.lines().next().unwrap_or_default();
    let plist = (!body.is_empty()).then(|| bplist::decode(body).ok()).flatten();
    match &plist {
        Some(p) => log_debug!("airplay events: {line}: {p:?}"),
        None => log_debug!("airplay events: {line} ({} byte body)", body.len()),
    }
    if let Some(remote) = plist.as_ref().and_then(remote_command) {
        log_debug!("airplay events: -> {remote:?}");
        dacp::deliver(remote);
    }
    header(&head, "CSeq").unwrap_or("0").to_string()
}

/// A volume or transport change the receiver reports, if this message is one.
fn remote_command(plist: &Value) -> Option<Remote> {
    if let Some(volume) = find(plist, "volume").and_then(number) {
        // dB (-144 / -30..0) like DACP, or already a 0..1 fraction.
        return Some(if volume <= 0.0 { Remote::Volume(volume::from_db(volume as f32)) } else { Remote::Volume((volume as f32).min(1.0)) });
    }
    let command = find(plist, "command").or_else(|| find(plist, "mediaRemoteCommand")).and_then(number)?;
    // MediaRemote command ids: 0 play, 1 pause, 2 toggle, 4 next, 5 previous.
    Some(match command as i64 {
        0 => Remote::Play,
        1 => Remote::Pause,
        2 => Remote::PlayPause,
        4 => Remote::Next,
        5 => Remote::Prev,
        _ => return None,
    })
}

/// First value stored under `key` anywhere in the plist.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v).or_else(|| d.iter().find_map(|(_, v)| find(v, key))),
        Value::Array(a) => a.iter().find_map(|v| find(v, key)),
        _ => None,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Real(r) => Some(*r),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::airplay::ap2::bplist::dict;

    #[test]
    fn requests_split_on_content_length() {
        let req = b"POST /command RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 2\r\n\r\nhiPOST";
        assert_eq!(request_len(req), Some(req.len() - 4));
        assert_eq!(request_len(b"POST /command RTSP/1.0\r\nContent-Length: 9\r\n\r\nhi"), None);
    }

    #[test]
    fn volume_and_commands() {
        let v = dict([("type", Value::String("x".into())), ("params", dict([("volume", Value::Real(-15.0))]))]);
        assert_eq!(remote_command(&v), Some(Remote::Volume(0.5)));
        assert_eq!(remote_command(&dict([("volume", Value::Real(0.25))])), Some(Remote::Volume(0.25)));
        assert_eq!(remote_command(&dict([("command", Value::Int(4))])), Some(Remote::Next));
        assert_eq!(remote_command(&dict([("type", Value::String("updateMRSupportedCommands".into()))])), None);
    }

    #[test]
    fn decrypts_with_either_key() {
        let key = [7u8; 32];
        let sealed = Encryptor::new(&key).seal(b"POST /x RTSP/1.0\r\n\r\n");
        let len = [sealed[0], sealed[1]];
        assert_eq!(open(&key, 0, &len, &sealed[2..]).as_deref(), Some(&b"POST /x RTSP/1.0\r\n\r\n"[..]));
        assert!(open(&[8u8; 32], 0, &len, &sealed[2..]).is_none());
    }
}
