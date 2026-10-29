//! The network threads of a session: audio pacing, timing replies, retransmits.

use super::packets;
use super::{FRAMES_PER_PACKET, HISTORY, LATENCY, LEAD, RATE};
use chacha20poly1305::ChaCha20Poly1305;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// State shared between the sink (control) and its threads.
pub struct Shared {
    pub running: AtomicBool,
    pub playing: AtomicBool,
    /// Re-anchor the clock and mark the next packet as a stream start.
    pub restart: AtomicBool,
    /// Discard buffered audio.
    pub drain: AtomicBool,
    /// The audio thread is parked (not sending).
    pub idle: AtomicBool,
    /// The session is unusable (the network path to the receiver went away, e.g.
    /// the local address vanished): the player should reconnect.
    pub broken: AtomicBool,
    /// Next RTP sequence number / timestamp.
    pub seq: AtomicU16,
    pub rtptime: AtomicU32,
    history: Mutex<VecDeque<(u16, Vec<u8>)>>,
    pub stats: Stats,
}

/// Diagnostics, logged (debug level) every [`STATS_EVERY`] and when a session ends.
#[derive(Default)]
pub struct Stats {
    pub timing_requests: AtomicU64,
    /// Packets the receiver asked for again, and how many of those we still had.
    pub requested: AtomicU64,
    pub resent: AtomicU64,
    pub packets: AtomicU64,
    pub audible: AtomicU64,
    /// Packets padded with silence because the player had nothing ready.
    pub padded: AtomicU64,
}

impl Stats {
    pub fn summary(&self) -> String {
        let n = |c: &AtomicU64| c.load(Ordering::Relaxed);
        format!(
            "{} packets sent ({} audible, {} padded), {} retransmits asked ({} resent), {} timing requests",
            n(&self.packets),
            n(&self.audible),
            n(&self.padded),
            n(&self.requested),
            n(&self.resent),
            n(&self.timing_requests)
        )
    }
}

/// Falling this far behind real time (frames) means packets would reach the receiver
/// after their play time, so the stream is re-anchored instead of caught up.
const STALL: u64 = LATENCY as u64 * 3 / 4;

/// How often a playing session logs its [`Stats`].
const STATS_EVERY: Duration = Duration::from_secs(30);

impl Shared {
    pub fn new(seq: u16, rtptime: u32) -> Self {
        Self {
            running: AtomicBool::new(true),
            playing: AtomicBool::new(true),
            restart: AtomicBool::new(true),
            drain: AtomicBool::new(false),
            idle: AtomicBool::new(false),
            broken: AtomicBool::new(false),
            seq: AtomicU16::new(seq),
            rtptime: AtomicU32::new(rtptime),
            history: Mutex::new(VecDeque::with_capacity(HISTORY)),
            stats: Stats::default(),
        }
    }

    fn remember(&self, seq: u16, packet: Vec<u8>) {
        let mut h = self.history.lock().unwrap();
        if h.len() == HISTORY {
            h.pop_front();
        }
        h.push_back((seq, packet));
    }
}

pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn thread")
}

/// Answer the receiver's NTP timing requests.
pub fn timing_loop(sock: UdpSocket, sh: &Shared) {
    let mut buf = [0u8; 128];
    while sh.running.load(Ordering::Relaxed) {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        if let Some(reply) = packets::timing_reply(&buf[..n]) {
            sh.stats.timing_requests.fetch_add(1, Ordering::Relaxed);
            let _ = sock.send_to(&reply, from);
        }
    }
}

/// Resend packets the receiver reports missing.
pub fn control_loop(sock: UdpSocket, dst: SocketAddr, sh: &Shared) {
    let mut buf = [0u8; 64];
    while sh.running.load(Ordering::Relaxed) {
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let Some((first, count)) = packets::retransmit_request(&buf[..n]) else { continue };
        sh.stats.requested.fetch_add(count as u64, Ordering::Relaxed);
        let history = sh.history.lock().unwrap();
        for seq in (0..count).map(|i| first.wrapping_add(i)) {
            if let Some((_, packet)) = history.iter().find(|(s, _)| *s == seq) {
                let _ = sock.send_to(&packets::retransmission(seq, packet), dst);
                sh.stats.resent.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// How the audio thread keeps the receiver's clock aligned.
pub enum Timing {
    Ntp,
    Ptp { clock_id: u64 },
}

pub struct AudioOut {
    pub socket: UdpSocket,
    pub control: UdpSocket,
    pub control_dst: SocketAddr,
    pub cipher: Option<ChaCha20Poly1305>,
    pub timing: Timing,
}

/// Send audio at real-time pace (slightly ahead), with a sync packet every second.
/// Pads with silence on underrun so the receiver's clock keeps running.
pub fn audio_loop(out: AudioOut, mut consumer: rtrb::Consumer<i16>, sh: &Shared) {
    crate::awake::high_priority_thread();
    let ssrc = fastrand::u32(..);
    let mut frame = vec![0i16; FRAMES_PER_PACKET * 2];
    let mut start = Instant::now();
    let mut start_rtp = 0u32;
    let mut sent: u64 = 0;
    let mut next_sync: u64 = 0;
    let mut first = true;
    let mut last_stats = Instant::now();
    while sh.running.load(Ordering::Relaxed) {
        if sh.drain.swap(false, Ordering::AcqRel) {
            let n = consumer.slots();
            if let Ok(c) = consumer.read_chunk(n) {
                c.commit_all();
            }
        }
        if !sh.playing.load(Ordering::Acquire) {
            sh.idle.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        sh.idle.store(false, Ordering::Release);
        if last_stats.elapsed() >= STATS_EVERY {
            last_stats = Instant::now();
            log_debug!("airplay: {}", sh.stats.summary());
        }
        if sh.restart.swap(false, Ordering::AcqRel) {
            start = Instant::now();
            start_rtp = sh.rtptime.load(Ordering::Acquire);
            sent = 0;
            next_sync = 0;
            first = true;
        }
        let elapsed = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
        // The thread didn't get to run for a while (the OS throttled or starved it).
        // Rushing out the backlog would only deliver packets the receiver throws away
        // as late, so start the clock again from here: a short gap, then the song
        // continues where it was.
        if elapsed > sent + STALL {
            log_warn!("airplay: audio fell {} ms behind real time, re-syncing", (elapsed - sent) * 1000 / RATE as u64);
            sh.restart.store(true, Ordering::Release);
            continue;
        }
        if elapsed >= next_sync {
            let now_rtp = start_rtp.wrapping_add(elapsed as u32);
            let _ = match out.timing {
                Timing::Ptp { clock_id } => {
                    out.control.send_to(&packets::sync_ptp(now_rtp, sh.rtptime.load(Ordering::Acquire), clock_id, first), out.control_dst)
                }
                Timing::Ntp => out.control.send_to(&packets::sync_ntp(now_rtp, first), out.control_dst),
            };
            next_sync = elapsed + RATE as u64;
        }
        if sent >= elapsed + LEAD {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        if !fill_frame(&mut consumer, &mut frame) {
            sh.stats.padded.fetch_add(1, Ordering::Relaxed);
        }

        let seq = sh.seq.load(Ordering::Acquire);
        let rtptime = sh.rtptime.load(Ordering::Acquire);
        let packet = packets::audio_packet(seq, rtptime, ssrc, first, &frame, out.cipher.as_ref());
        if let Err(e) = out.socket.send(&packet)
            && !sh.broken.swap(true, Ordering::AcqRel)
        {
            log_warn!("airplay send failed, session lost: {e}");
        }
        sh.stats.packets.fetch_add(1, Ordering::Relaxed);
        if frame.iter().any(|&s| s != 0) {
            sh.stats.audible.fetch_add(1, Ordering::Relaxed);
        }
        sh.remember(seq, packet);
        first = false;
        sent += FRAMES_PER_PACKET as u64;
        sh.seq.store(seq.wrapping_add(1), Ordering::Release);
        sh.rtptime.store(rtptime.wrapping_add(FRAMES_PER_PACKET as u32), Ordering::Release);
    }
}

/// Take a whole packet's worth of samples if available; pad with silence. Returns
/// whether the packet was full.
fn fill_frame(consumer: &mut rtrb::Consumer<i16>, frame: &mut [i16]) -> bool {
    let available = consumer.slots().min(frame.len()) & !1;
    if let Ok(chunk) = consumer.read_chunk(available) {
        let (a, b) = chunk.as_slices();
        frame[..a.len()].copy_from_slice(a);
        frame[a.len()..a.len() + b.len()].copy_from_slice(b);
        chunk.commit_all();
    }
    frame[available..].fill(0);
    available == frame.len()
}
