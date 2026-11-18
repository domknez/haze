//! DACP remote-control server. AirPlay receivers send their hardware button presses
//! (volume, play/pause, skip) back to the sender: they look up
//! `iTunes_Ctrl_<DACP-ID>._dacp._tcp` via mDNS and issue plain HTTP requests such as
//! `GET /ctrl-int/1/setproperty?dmcp.device-volume=-12.5`.
//!
//! The server listens on every interface, so a press is taken only from a speaker
//! haze plays on: one that comes from its address, or carries the `Active-Remote`
//! haze gave it.

use crate::sync::Lock;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Process-wide DACP id, sent to receivers as `DACP-ID` / `Client-Instance`.
pub fn id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("{:016X}", fastrand::u64(..)))
}

/// Sent to receivers as `Active-Remote`; they send it back with every press.
pub fn active_remote() -> &'static str {
    static ACTIVE_REMOTE: OnceLock<String> = OnceLock::new();
    ACTIVE_REMOTE.get_or_init(|| fastrand::u32(..).to_string())
}

/// Addresses of the speakers haze plays on now, one entry per session.
static SPEAKERS: Mutex<Vec<IpAddr>> = Mutex::new(Vec::new());

/// A speaker whose presses are taken while this is held (for its session).
pub struct Speaker(IpAddr);

impl Speaker {
    pub fn new(ip: IpAddr) -> Self {
        let ip = ip.to_canonical();
        SPEAKERS.locked().push(ip);
        Speaker(ip)
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        let mut speakers = SPEAKERS.locked();
        if let Some(i) = speakers.iter().position(|ip| *ip == self.0) {
            speakers.remove(i);
        }
    }
}

/// Whether a press comes from a speaker haze plays on: from its address, or with
/// the `Active-Remote` haze gave it.
fn trusted(peer: Option<IpAddr>, active: Option<&str>, speakers: &[IpAddr]) -> bool {
    peer.is_some_and(|ip| speakers.contains(&ip.to_canonical())) || active == Some(active_remote())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Remote {
    /// 0.0 ..= 1.0
    Volume(f32),
    VolumeStep(f32),
    PlayPause,
    Play,
    Pause,
    Next,
    Prev,
}

fn parse(path: &str) -> Option<Remote> {
    let cmd = path.strip_prefix("/ctrl-int/1/")?;
    let (name, query) = cmd.split_once('?').unwrap_or((cmd, ""));
    Some(match name {
        "setproperty" => {
            let db = query.split('&').find_map(|kv| kv.strip_prefix("dmcp.device-volume="))?.parse::<f32>().ok()?;
            Remote::Volume(super::volume::from_db(db))
        }
        "volumeup" => Remote::VolumeStep(1.0 / 16.0),
        "volumedown" => Remote::VolumeStep(-1.0 / 16.0),
        "playpause" => Remote::PlayPause,
        "play" | "playresume" => Remote::Play,
        "pause" | "stop" => Remote::Pause,
        "nextitem" | "beginff" => Remote::Next,
        "previtem" | "beginrew" => Remote::Prev,
        _ => return None,
    })
}

/// Where button presses go: set by [`start`], also fed by the AirPlay 2 event channel.
type Handler = Box<dyn Fn(Remote) + Send>;
static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);

/// Hand a button press to the app (no-op before [`start`]).
pub fn deliver(remote: Remote) {
    if let Some(handler) = HANDLER.locked().as_ref() {
        handler(remote);
    }
}

/// Our mDNS advertisement, shared so a new AirPlay session can narrow it down.
struct Advert {
    daemon: ServiceDaemon,
    port: u16,
    fullname: String,
}

static ADVERT: Mutex<Option<Advert>> = Mutex::new(None);

impl Advert {
    fn register(&mut self, ip: IpAddr) -> Result<(), String> {
        let host = format!("haze-{}.local.", id()[..8].to_ascii_lowercase());
        let props = [("txtvers", "1"), ("Ver", "131075"), ("DbId", id()), ("OSsi", "0x1F5")];
        let name = format!("iTunes_Ctrl_{}", id());
        let info = ServiceInfo::new("_dacp._tcp.local.", &name, &host, ip, self.port, &props[..]).map_err(|e| e.to_string())?;
        self.fullname = info.get_fullname().to_string();
        self.daemon.register(info).map_err(|e| e.to_string())
    }

    fn unregister(&self) {
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(Duration::from_millis(500));
        }
    }
}

/// Advertise the remote-control service only on the interface that reaches the
/// receiver we just connected to, with just that address, and announce it now.
/// Advertising every interface (VPNs, container bridges, loopback) made receivers
/// such as Sonos take ~25 s to reach us, so button presses arrived in late bursts.
pub fn advertise_on(local_ip: IpAddr) {
    let mut advert = ADVERT.locked();
    let Some(advert) = advert.as_mut() else { return };
    let _ = advert.daemon.disable_interface(IfKind::All);
    let _ = advert.daemon.enable_interface(IfKind::Addr(local_ip));
    if !advert.fullname.is_empty() {
        advert.unregister();
    }
    match advert.register(local_ip) {
        Ok(()) => log_debug!("dacp: advertised on {local_ip}:{}", advert.port),
        Err(e) => log_warn!("dacp: re-advertising failed: {e}"),
    }
}

/// Keeps the remote-control service running; withdraws its mDNS record on drop,
/// so receivers don't keep a dead remote around.
pub struct Server;

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(advert) = ADVERT.locked().take() {
            if !advert.fullname.is_empty() {
                advert.unregister();
            }
            let _ = advert.daemon.shutdown();
        }
    }
}

/// Start listening and advertise ourselves. `on_command` runs on the server thread.
pub fn start(on_command: impl Fn(Remote) + Send + 'static) -> Result<Server, String> {
    *HANDLER.locked() = Some(Box::new(on_command));
    let listener = TcpListener::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
    // Not advertised until an AirPlay session starts (see `advertise_on`): an
    // all-interfaces record would be cached by receivers, which then try its
    // unreachable addresses first.
    let advert = Advert { daemon, port, fullname: String::new() };
    *ADVERT.locked() = Some(advert);

    std::thread::Builder::new()
        .name("dacp".into())
        .spawn(move || {
            // A connection per receiver, kept open: they reuse it for later presses.
            let open = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming().flatten() {
                log_debug!("dacp: connection from {:?}", stream.peer_addr().ok());
                if open.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                    log_warn!("dacp: too many connections, refusing one");
                    continue;
                }
                open.fetch_add(1, Ordering::AcqRel);
                let done = open.clone();
                let spawned = std::thread::Builder::new().name("dacp-conn".into()).spawn(move || {
                    handle(stream);
                    done.fetch_sub(1, Ordering::AcqRel);
                });
                if spawned.is_err() {
                    open.fetch_sub(1, Ordering::AcqRel);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Server)
}

/// Receivers keep one connection each; more than this is someone else.
const MAX_CONNECTIONS: usize = 8;
/// Longer request or header lines than this aren't a receiver's.
const MAX_LINE: u64 = 4096;

/// Read a line of at most [`MAX_LINE`] bytes; false at the end, on an error or on a
/// longer line.
fn read_line(reader: &mut impl BufRead, line: &mut String) -> bool {
    line.clear();
    matches!(reader.by_ref().take(MAX_LINE).read_line(line), Ok(n) if n > 0 && line.ends_with('\n'))
}

fn handle(stream: TcpStream) {
    let peer = stream.peer_addr().ok().map(|a| a.ip());
    stream.set_read_timeout(Some(Duration::from_secs(600))).ok();
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut writer = stream;
    let (mut request, mut line) = (String::new(), String::new());
    // Keep-alive: receivers may send several requests on one connection.
    loop {
        if !read_line(&mut reader, &mut request) {
            return;
        }
        let mut active = None;
        loop {
            if !read_line(&mut reader, &mut line) {
                return;
            }
            if line.trim_end().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("Active-Remote")
            {
                active = Some(value.trim().to_string());
            }
        }
        let path = request.split_whitespace().nth(1).unwrap_or("");
        let command = parse(path);
        let status = match command {
            Some(c) if trusted(peer, active.as_deref(), &SPEAKERS.locked()) => {
                log_debug!("dacp: {path} -> {c:?}");
                deliver(c);
                "204 No Content"
            }
            Some(_) => {
                log_warn!("dacp: refused {path} from {peer:?} (Active-Remote {active:?}): not a speaker haze plays on");
                "403 Forbidden"
            }
            None => "404 Not Found",
        };
        let resp = format!("HTTP/1.1 {status}\r\nDAAP-Server: haze\r\nContent-Length: 0\r\n\r\n");
        if writer.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_presses_only_from_the_speaker_played_on() {
        let speaker: IpAddr = [192, 168, 1, 20].into();
        let other: IpAddr = [192, 168, 1, 99].into();
        let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
        assert!(trusted(Some(speaker), None, &[speaker]));
        assert!(trusted(Some(mapped), None, &[speaker]), "IPv4 over IPv6");
        assert!(trusted(Some(other), Some(active_remote()), &[speaker]), "its Active-Remote");
        assert!(!trusted(Some(other), None, &[speaker]));
        assert!(!trusted(Some(other), Some("1"), &[speaker]));
        assert!(!trusted(Some(speaker), None, &[]), "no session");
    }

    #[test]
    fn reads_lines_up_to_a_limit() {
        let long = format!("GET /{} HTTP/1.1\r\n", "x".repeat(MAX_LINE as usize));
        let mut line = String::new();
        let mut ok = BufReader::new("GET /ctrl-int/1/playpause HTTP/1.1\r\nActive-Remote: 1\r\n\r\n".as_bytes());
        assert!(read_line(&mut ok, &mut line) && line.starts_with("GET /ctrl-int"));
        assert!(read_line(&mut ok, &mut line) && line.starts_with("Active-Remote"));
        assert!(read_line(&mut ok, &mut line) && line.trim_end().is_empty());
        assert!(!read_line(&mut ok, &mut line), "the end");
        assert!(!read_line(&mut BufReader::new(long.as_bytes()), &mut line), "too long");
    }

    #[test]
    fn parses_receiver_requests() {
        assert_eq!(parse("/ctrl-int/1/setproperty?dmcp.device-volume=-15.000000"), Some(Remote::Volume(0.5)));
        assert_eq!(parse("/ctrl-int/1/setproperty?dmcp.device-volume=-144.0"), Some(Remote::Volume(0.0)));
        assert_eq!(parse("/ctrl-int/1/setproperty?dmcp.device-volume=0"), Some(Remote::Volume(1.0)));
        assert_eq!(parse("/ctrl-int/1/nextitem"), Some(Remote::Next));
        assert_eq!(parse("/ctrl-int/1/playpause"), Some(Remote::PlayPause));
        assert_eq!(parse("/login"), None);
    }
}
