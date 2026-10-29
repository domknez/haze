//! Minimal PTP (IEEE 1588v2) grandmaster for AirPlay 2 receivers, modelled on
//! OwnTone's libairptp (MIT). Not a real disciplined clock: we announce ourselves
//! as a GPS-quality grandmaster so receivers follow us, and unicast
//! Announce/Sync/Follow_Up/Signaling to every active receiver, answering Delay_Req.
//!
//! Binds UDP 319 (event) and 320 (general). Fine without root on macOS and Windows;
//! Linux needs CAP_NET_BIND_SERVICE or a lowered `ip_unprivileged_port_start`.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const EVENT_PORT: u16 = 319;
const GENERAL_PORT: u16 = 320;
const SYNC_EVERY: Duration = Duration::from_millis(125);
const ANNOUNCE_EVERY: Duration = Duration::from_secs(1);

const MSG_SYNC: u8 = 0x00;
const MSG_DELAY_REQ: u8 = 0x01;
const MSG_FOLLOW_UP: u8 = 0x08;
const MSG_DELAY_RESP: u8 = 0x09;
const MSG_ANNOUNCE: u8 = 0x0b;
const MSG_SIGNALING: u8 = 0x0c;

const FLAG_TIMESCALE: u16 = 1 << 3;
const FLAG_TWO_STEP: u16 = 1 << 9;
const FLAG_UNICAST: u16 = 1 << 10;

const ORG_IEEE: [u8; 3] = [0x00, 0x80, 0xc2];
const ORG_APPLE: [u8; 3] = [0x00, 0x0d, 0x93];
const TLV_ORG_EXTENSION: u16 = 0x0003;
const TLV_PATH_TRACE: u16 = 0x0008;

/// Our PTP timescale: nanoseconds since this process first needed a clock
/// (plus a constant so it is never near zero). Receivers only need consistency.
pub fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000_000
}

pub struct PtpMaster {
    pub clock_id: u64,
    peers: Mutex<Vec<(IpAddr, usize)>>,
    event: UdpSocket,
    general: UdpSocket,
}

/// Registration of one receiver; unregisters on drop. The master shuts down
/// once the last receiver is gone.
pub struct PtpPeer {
    master: Arc<PtpMaster>,
    ip: IpAddr,
}

impl Drop for PtpPeer {
    fn drop(&mut self) {
        let mut peers = self.master.peers.lock().unwrap();
        if let Some(i) = peers.iter().position(|(ip, _)| *ip == self.ip) {
            peers[i].1 -= 1;
            if peers[i].1 == 0 {
                peers.remove(i);
            }
        }
    }
}

impl PtpPeer {
    pub fn clock_id(&self) -> u64 {
        self.master.clock_id
    }
}

/// Shared master, started on first use.
pub fn join(ip: IpAddr) -> Result<PtpPeer, String> {
    static MASTER: Mutex<Weak<PtpMaster>> = Mutex::new(Weak::new());
    let mut slot = MASTER.lock().unwrap();
    let master = match slot.upgrade() {
        Some(m) => m,
        None => {
            let m = PtpMaster::start()?;
            *slot = Arc::downgrade(&m);
            m
        }
    };
    {
        let mut peers = master.peers.lock().unwrap();
        match peers.iter_mut().find(|(p, _)| *p == ip) {
            Some((_, n)) => *n += 1,
            None => peers.push((ip, 1)),
        }
    }
    master.send_round(0, true); // prime the receiver before SETUP
    Ok(PtpPeer { master, ip })
}

impl PtpMaster {
    fn start() -> Result<Arc<Self>, String> {
        let bind = |port| {
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port)))
                .map_err(|e| format!("PTP needs UDP port {port} ({e}); on Linux run with CAP_NET_BIND_SERVICE"))
        };
        let event = bind(EVENT_PORT)?;
        let general = bind(GENERAL_PORT)?;
        event.set_read_timeout(Some(Duration::from_millis(50))).ok();
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).expect("OS RNG");
        let master = Arc::new(Self { clock_id: u64::from_be_bytes(id), peers: Mutex::new(Vec::new()), event, general });

        let weak = Arc::downgrade(&master);
        std::thread::Builder::new()
            .name("ptp-master".into())
            .spawn(move || {
                let mut seq: u16 = 1;
                let mut next_announce = Instant::now();
                let mut next_sync = Instant::now();
                let mut buf = [0u8; 256];
                // Runs while any PtpPeer keeps the master alive.
                while let Some(m) = weak.upgrade() {
                    let now = Instant::now();
                    if now >= next_sync {
                        let announce = now >= next_announce;
                        m.send_round(seq, announce);
                        if announce {
                            next_announce = now + ANNOUNCE_EVERY;
                        }
                        seq = seq.wrapping_add(1);
                        next_sync = now + SYNC_EVERY;
                    }
                    // Delay_Req arrives on the event port; answer on general.
                    if let Ok((n, from)) = m.event.recv_from(&mut buf) {
                        let rx = now_ns();
                        if n >= 44 && buf[0] & 0x0f == MSG_DELAY_REQ {
                            let req_seq = u16::from_be_bytes([buf[30], buf[31]]);
                            let port_id: [u8; 10] = buf[20..30].try_into().unwrap();
                            let resp = m.delay_resp(req_seq, &port_id, rx);
                            let _ = m.general.send_to(&resp, SocketAddr::new(from.ip(), GENERAL_PORT));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(master)
    }

    fn send_round(&self, seq: u16, announce: bool) {
        let peers: Vec<IpAddr> = self.peers.lock().unwrap().iter().map(|(ip, _)| *ip).collect();
        if announce {
            let a = self.announce(seq);
            let s = self.signaling(seq);
            for ip in &peers {
                let _ = self.general.send_to(&a, SocketAddr::new(*ip, GENERAL_PORT));
                let _ = self.general.send_to(&s, SocketAddr::new(*ip, GENERAL_PORT));
            }
        }
        // Two-step: Sync without timestamp, then Follow_Up with the send time.
        let sync = self.sync(seq);
        let ts = now_ns();
        for ip in &peers {
            let _ = self.event.send_to(&sync, SocketAddr::new(*ip, EVENT_PORT));
        }
        let fu = self.follow_up(seq, ts);
        for ip in &peers {
            let _ = self.general.send_to(&fu, SocketAddr::new(*ip, GENERAL_PORT));
        }
    }

    fn header(&self, msg: u8, len: usize, seq: u16, log_interval: i8, flags: u16) -> Vec<u8> {
        let mut h = Vec::with_capacity(len);
        h.push(msg | 0x10); // transportSpecific = 1, as nqptp and Apple expect
        h.push(0x02); // PTPv2
        h.extend_from_slice(&(len as u16).to_be_bytes());
        h.push(0); // domain
        h.push(0);
        h.extend_from_slice(&flags.to_be_bytes());
        h.extend_from_slice(&[0; 8]); // correction
        h.extend_from_slice(&[0; 4]);
        h.extend_from_slice(&self.clock_id.to_be_bytes());
        h.extend_from_slice(&[0x80, 0x05]); // port number, same as iOS
        h.extend_from_slice(&seq.to_be_bytes());
        h.push(if msg == MSG_SIGNALING { 0x05 } else { 0x00 }); // control field
        h.push(log_interval as u8);
        h
    }

    fn sync(&self, seq: u16) -> Vec<u8> {
        let mut m = self.header(MSG_SYNC, 44, seq, -3, FLAG_UNICAST | FLAG_TIMESCALE | FLAG_TWO_STEP);
        m.extend_from_slice(&[0; 10]);
        m
    }

    fn follow_up(&self, seq: u16, ts: u64) -> Vec<u8> {
        let mut m = self.header(MSG_FOLLOW_UP, 44 + 32 + 20, seq, -3, FLAG_UNICAST | FLAG_TIMESCALE);
        m.extend_from_slice(&timestamp(ts));
        // IEEE 802.1AS Follow_Up information TLV (all-zero rate/phase info).
        let mut v = [0u8; 28];
        v[..3].copy_from_slice(&ORG_IEEE);
        v[3..6].copy_from_slice(&[0, 0, 1]);
        tlv(&mut m, TLV_ORG_EXTENSION, &v);
        // Apple clock-id TLV.
        let mut v = [0u8; 16];
        v[..3].copy_from_slice(&ORG_APPLE);
        v[3..6].copy_from_slice(&[0, 0, 4]);
        v[6..14].copy_from_slice(&self.clock_id.to_be_bytes());
        tlv(&mut m, TLV_ORG_EXTENSION, &v);
        m
    }

    fn announce(&self, seq: u16) -> Vec<u8> {
        let mut m = self.header(MSG_ANNOUNCE, 64 + 12, seq, 0, FLAG_UNICAST | FLAG_TIMESCALE);
        m.extend_from_slice(&[0; 10]); // origin timestamp: iOS sends 0
        m.extend_from_slice(&0i16.to_be_bytes()); // UTC offset
        m.push(0);
        m.push(128); // priority1
        m.extend_from_slice(&(0x0621_0000u32 | 0x436a).to_be_bytes()); // class 6 (GPS), 100 ns
        m.push(128); // priority2
        m.extend_from_slice(&self.clock_id.to_be_bytes());
        m.extend_from_slice(&0u16.to_be_bytes()); // steps removed
        m.push(0x20); // time source: GPS
        tlv(&mut m, TLV_PATH_TRACE, &self.clock_id.to_be_bytes());
        m
    }

    fn signaling(&self, seq: u16) -> Vec<u8> {
        let mut m = self.header(MSG_SIGNALING, 44 + 26 + 36, seq, -128, FLAG_UNICAST | FLAG_TIMESCALE);
        m.extend_from_slice(&[0; 10]); // target port identity
        for (sub, len) in [([0, 0, 1], 22), ([0, 0, 5], 32)] {
            let mut v = vec![0u8; len];
            v[..3].copy_from_slice(&ORG_APPLE);
            v[3..6].copy_from_slice(&sub);
            v[6..10].copy_from_slice(&[0x00, 0x00, 0x03, 0x01]);
            tlv(&mut m, TLV_ORG_EXTENSION, &v);
        }
        m
    }

    fn delay_resp(&self, seq: u16, requester: &[u8; 10], rx: u64) -> Vec<u8> {
        let mut m = self.header(MSG_DELAY_RESP, 54, seq, -3, FLAG_UNICAST | FLAG_TIMESCALE | FLAG_TWO_STEP);
        m.extend_from_slice(&timestamp(rx));
        m.extend_from_slice(requester);
        m
    }
}

fn timestamp(ns: u64) -> [u8; 10] {
    let secs = ns / 1_000_000_000;
    let nanos = (ns % 1_000_000_000) as u32;
    let mut t = [0u8; 10];
    t[..2].copy_from_slice(&((secs >> 32) as u16).to_be_bytes());
    t[2..6].copy_from_slice(&(secs as u32).to_be_bytes());
    t[6..].copy_from_slice(&nanos.to_be_bytes());
    t
}

fn tlv(m: &mut Vec<u8>, t: u16, value: &[u8]) {
    m.extend_from_slice(&t.to_be_bytes());
    m.extend_from_slice(&(value.len() as u16).to_be_bytes());
    m.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> PtpMaster {
        let s = || UdpSocket::bind("127.0.0.1:0").unwrap();
        PtpMaster { clock_id: 0x1122_3344_5566_7788, peers: Mutex::new(Vec::new()), event: s(), general: s() }
    }

    #[test]
    fn message_sizes_match_reference_structs() {
        let m = master();
        for (msg, len) in
            [(m.sync(1), 44), (m.follow_up(1, 5), 96), (m.announce(1), 76), (m.signaling(1), 106), (m.delay_resp(1, &[0; 10], 5), 54)]
        {
            assert_eq!(msg.len(), len);
            assert_eq!(u16::from_be_bytes([msg[2], msg[3]]) as usize, len);
        }
        assert_eq!(&m.announce(1)[20..28], &0x1122_3344_5566_7788u64.to_be_bytes());
    }

    #[test]
    fn timestamp_layout() {
        let t = timestamp(5_000_000_123);
        assert_eq!(u32::from_be_bytes(t[2..6].try_into().unwrap()), 5);
        assert_eq!(u32::from_be_bytes(t[6..10].try_into().unwrap()), 123);
    }
}
