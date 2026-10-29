//! Session handshakes: AirPlay 1 (RAOP: ANNOUNCE/SETUP/RECORD with SDP) and
//! AirPlay 2 (transient pairing, encrypted plist SETUPs, PTP or NTP timing).

use super::ap2::bplist::{self, Value, dict};
use super::ap2::pairing::{self, SrpClient, tlv};
use super::ap2::ptp::{self, PtpPeer};
use super::discovery::Device;
use super::rtsp::{Rtsp, transport_param};
use super::{FRAMES_PER_PACKET, LATENCY, MIN_LATENCY, RATE};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

const PLIST: &str = "application/x-apple-binary-plist";
const OCTETS: &str = "application/octet-stream";

/// Our side of the session.
pub struct Local {
    pub ip: IpAddr,
    pub session_id: u32,
    pub control_port: u16,
    pub timing_port: u16,
    /// First RTP sequence number / timestamp we will send.
    pub seq: u16,
    pub rtptime: u32,
}

/// What the receiver agreed to.
pub struct Negotiated {
    pub data_port: u16,
    pub control_port: u16,
    /// AirPlay 2 per-packet audio key (`shk`).
    pub audio_key: Option<[u8; 32]>,
    /// AirPlay 2 reverse event channel; must stay open for the session.
    pub events: Option<TcpStream>,
    pub ptp: Option<PtpPeer>,
}

pub fn airplay1(rtsp: &mut Rtsp, device: &Device, local: &Local, remote_ip: IpAddr) -> Result<Negotiated, String> {
    rtsp.request("OPTIONS", Some("*"), &[], None)?;
    if device.auth_setup {
        // MFi-SAP handshake: type byte 0x01 + a Curve25519 public key. Receivers
        // only need it to happen; the reply (their key + cert) is not used for RAOP.
        let mut body = vec![0x01];
        body.extend((0..32).map(|_| fastrand::u8(..)));
        rtsp.request("POST", Some("/auth-setup"), &[], Some((OCTETS, &body)))?;
    }
    let family = |ip: IpAddr| if ip.is_ipv4() { "IP4" } else { "IP6" };
    let sdp = format!(
        "v=0\r\no=iTunes {} 0 IN {} {}\r\ns=iTunes\r\nc=IN {} {remote_ip}\r\nt=0 0\r\nm=audio 0 RTP/AVP 96\r\na=rtpmap:96 AppleLossless\r\na=fmtp:96 {FRAMES_PER_PACKET} 0 16 40 10 14 2 255 0 0 {RATE}\r\n",
        local.session_id,
        family(local.ip),
        local.ip,
        family(remote_ip)
    );
    rtsp.request("ANNOUNCE", None, &[], Some(("application/sdp", sdp.as_bytes())))?;
    let transport =
        format!("RTP/AVP/UDP;unicast;interleaved=0-1;mode=record;control_port={};timing_port={}", local.control_port, local.timing_port);
    let resp = rtsp.request("SETUP", None, &[("Transport", transport)], None)?;
    rtsp.adopt_session(&resp);
    let t = resp.header("Transport").ok_or("SETUP: no Transport")?;
    let data_port = transport_param(t, "server_port").ok_or("SETUP: no server_port")?;
    let control_port = transport_param(t, "control_port").unwrap_or(data_port + 1);
    let rtp_info = format!("seq={};rtptime={}", local.seq, local.rtptime);
    rtsp.request("RECORD", None, &[("Range", "npt=0-".into()), ("RTP-Info", rtp_info)], None)?;
    Ok(Negotiated { data_port, control_port, audio_key: None, events: None, ptp: None })
}

pub fn airplay2(rtsp: &mut Rtsp, device: &Device, local: &Local, remote_ip: IpAddr) -> Result<Negotiated, String> {
    let key = pair_transient(rtsp)?;
    rtsp.encrypt(&key);

    // PTP when the receiver supports it (AirPlay 2 receivers generally won't play
    // NTP-timed streams); NTP only as a fallback, e.g. if ports 319/320 are taken.
    let ptp = device
        .ptp
        .then(|| ptp::join(remote_ip))
        .and_then(|joined| joined.inspect_err(|e| log_warn!("{e}; falling back to NTP timing")).ok());
    let resp = rtsp.request("SETUP", None, &[], Some((PLIST, &bplist::encode(&session_setup(local, ptp.as_ref())))))?;
    rtsp.adopt_session(&resp);
    let reply = bplist::decode(&resp.body)?;
    let events = reply
        .get("eventPort")
        .and_then(Value::as_u64)
        .filter(|&p| p > 0)
        .and_then(|p| TcpStream::connect_timeout(&SocketAddr::new(remote_ip, p as u16), Duration::from_secs(2)).ok());
    if let Some(reader) = events.as_ref().and_then(|s| s.try_clone().ok()) {
        super::ap2::events::spawn(reader, &key);
    }

    rtsp.request("RECORD", None, &[], None)?;
    if ptp.is_some() {
        let peers = Value::Array(vec![Value::String(remote_ip.to_string()), Value::String(local.ip.to_string())]);
        rtsp.request("SETPEERS", None, &[], Some(("/peer-list-changed", &bplist::encode(&peers))))?;
    }

    let audio_key: [u8; 32] = key[..32].try_into().expect("64-byte session key");
    let resp = rtsp.request("SETUP", None, &[], Some((PLIST, &bplist::encode(&stream_setup(local, &audio_key)))))?;
    let reply = bplist::decode(&resp.body)?;
    let stream = reply.get("streams").and_then(|s| s.index(0)).ok_or("SETUP stream: no streams in reply")?;
    let data_port = stream.get("dataPort").and_then(Value::as_u64).ok_or("SETUP stream: no dataPort")?;
    let control_port = stream.get("controlPort").and_then(Value::as_u64).ok_or("SETUP stream: no controlPort")?;
    Ok(Negotiated { data_port: data_port as u16, control_port: control_port as u16, audio_key: Some(audio_key), events, ptp })
}

/// Ask the receiver (`GET /info`) how long it takes to turn audio into sound, on top
/// of the latency we ask for. Senders of video use this to keep the picture in
/// step; receivers that don't answer, or don't say, get none. `rtsp` is a
/// connection of its own, so a receiver that dislikes the question can't spoil
/// the session.
pub fn info_latency(rtsp: &mut Rtsp, device: &Device) -> Option<Duration> {
    let resp = rtsp.request("GET", Some("/info"), &[], None).inspect_err(|e| log_debug!("airplay: {} /info: {e}", device.name)).ok()?;
    let info = bplist::decode(&resp.body).inspect_err(|e| log_debug!("airplay: {} /info: {e}", device.name)).ok()?;
    let latency = output_latency(&info);
    match latency {
        Some(l) => log_info!("airplay: {} reports {} ms output latency", device.name, l.as_millis()),
        None => log_info!("airplay: {} reports no output latency", device.name),
    }
    latency
}

/// From `/info`'s `audioLatencies`: the output latency for realtime streams
/// (type 96) of the default audio type, else for realtime streams, else any.
fn output_latency(info: &Value) -> Option<Duration> {
    let Some(Value::Array(entries)) = info.get("audioLatencies") else { return None };
    let is = |e: &&Value, kind: u64| e.get("type").and_then(Value::as_u64) == Some(kind);
    let default = |e: &&Value| matches!(e.get("audioType"), Some(Value::String(t)) if t == "default");
    let entry =
        entries.iter().find(|e| is(e, 96) && default(e)).or_else(|| entries.iter().find(|e| is(e, 96))).or_else(|| entries.first())?;
    let micros = entry.get("outputLatencyMicros").and_then(Value::as_u64)?;
    // Anything beyond a few seconds is a misreading, not a latency.
    (micros <= 5_000_000).then(|| Duration::from_micros(micros))
}

/// Transient HomeKit pair-setup (M1..M4). Returns the 64-byte SRP session key.
fn pair_transient(rtsp: &mut Rtsp) -> Result<[u8; 64], String> {
    let transient = [("X-Apple-HKP", "4".to_string())];
    let m1 = tlv::encode(&[(tlv::STATE, &[1]), (tlv::METHOD, &[0]), (tlv::FLAGS, &[0x10])]);
    let m2 = tlv::decode(&rtsp.request("POST", Some("/pair-setup"), &transient, Some((OCTETS, &m1)))?.body);
    if let Some(e) = tlv::get(&m2, tlv::ERROR) {
        return Err(format!("pair-setup refused (error {e:?})"));
    }
    let salt = tlv::get(&m2, tlv::SALT).ok_or("pair-setup: no salt")?;
    let server_key = tlv::get(&m2, tlv::PUBLIC_KEY).ok_or("pair-setup: no public key")?;
    let proof = SrpClient::new().process(salt, server_key, pairing::TRANSIENT_PIN)?;

    let m3 = tlv::encode(&[(tlv::STATE, &[3]), (tlv::PUBLIC_KEY, &proof.a_pub), (tlv::PROOF, &proof.m1)]);
    let m4 = tlv::decode(&rtsp.request("POST", Some("/pair-setup"), &transient, Some((OCTETS, &m3)))?.body);
    if let Some(e) = tlv::get(&m4, tlv::ERROR) {
        return Err(format!("pair-setup rejected our proof (error {e:?})"));
    }
    if tlv::get(&m4, tlv::PROOF) != Some(&proof.expected_m2[..]) {
        return Err("pair-setup: device proof mismatch".into());
    }
    Ok(proof.session_key)
}

/// First SETUP: who we are and how we keep time.
fn session_setup(local: &Local, ptp: Option<&PtpPeer>) -> Value {
    let device_id = random_bytes::<6>().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
    let session_uuid = uuid_v4();
    match ptp {
        Some(p) => {
            let peer = dict([
                ("ID", Value::String(session_uuid.clone())),
                ("DeviceType", Value::Int(0)),
                ("ClockID", Value::Int(p.clock_id())), // encoded as signed 64-bit, as Apple does
                ("SupportsClockPortMatchingOverride", Value::Bool(false)),
                ("Addresses", Value::Array(vec![Value::String(local.ip.to_string())])),
            ]);
            dict([
                ("name", Value::String("haze".into())),
                ("deviceID", Value::String(device_id.clone())),
                ("sessionUUID", Value::String(session_uuid)),
                ("timingProtocol", Value::String("PTP".into())),
                ("macAddress", Value::String(device_id)),
                ("groupUUID", Value::String(uuid_v4())),
                ("groupContainsGroupLeader", Value::Bool(false)),
                ("timingPeerInfo", peer.clone()),
                ("timingPeerList", Value::Array(vec![peer])),
            ])
        }
        None => dict([
            ("deviceID", Value::String(device_id)),
            ("sessionUUID", Value::String(session_uuid)),
            ("timingPort", Value::Int(local.timing_port as u64)),
            ("timingProtocol", Value::String("NTP".into())),
        ]),
    }
}

/// Second SETUP: a realtime ALAC stream.
fn stream_setup(local: &Local, audio_key: &[u8; 32]) -> Value {
    let stream = dict([
        ("audioFormat", Value::Int(0x40000)), // ALAC 44100/16/2
        ("audioMode", Value::String("default".into())),
        ("controlPort", Value::Int(local.control_port as u64)),
        ("ct", Value::Int(2)), // compression type: ALAC
        ("isMedia", Value::Bool(true)),
        ("latencyMax", Value::Int(LATENCY as u64)),
        ("latencyMin", Value::Int(MIN_LATENCY as u64)),
        ("shk", Value::Data(audio_key.to_vec())),
        ("spf", Value::Int(FRAMES_PER_PACKET as u64)),
        ("sr", Value::Int(RATE as u64)),
        ("type", Value::Int(96)), // realtime
        ("supportsDynamicStreamID", Value::Bool(false)),
        ("streamConnectionID", Value::Int(u32::from_be_bytes(random_bytes::<4>()) as u64)),
    ]);
    dict([("streams", Value::Array(vec![stream]))])
}

fn uuid_v4() -> String {
    let mut u = random_bytes::<16>();
    u[6] = (u[6] & 0x0f) | 0x40;
    u[8] = (u[8] & 0x3f) | 0x80;
    let hex: String = u.iter().map(|b| format!("{b:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS RNG");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latency(kind: u64, audio: &str, micros: u64) -> Value {
        dict([("type", Value::Int(kind)), ("audioType", Value::String(audio.into())), ("outputLatencyMicros", Value::Int(micros))])
    }

    #[test]
    fn reads_the_realtime_output_latency() {
        let info = dict([(
            "audioLatencies",
            Value::Array(vec![latency(100, "default", 400_000), latency(96, "media", 90_000), latency(96, "default", 120_000)]),
        )]);
        assert_eq!(output_latency(&info), Some(Duration::from_millis(120)));
        let no_default = dict([("audioLatencies", Value::Array(vec![latency(103, "default", 1), latency(96, "media", 90_000)]))]);
        assert_eq!(output_latency(&no_default), Some(Duration::from_millis(90)));
        assert_eq!(output_latency(&dict([("name", Value::String("Kitchen".into()))])), None);
        assert_eq!(output_latency(&dict([("audioLatencies", Value::Array(vec![latency(96, "default", 60_000_000)]))])), None);
    }
}
