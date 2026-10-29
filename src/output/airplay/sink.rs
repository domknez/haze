//! [`AirPlaySink`]: connects to a speaker and implements the audio [`Sink`]. Its
//! control requests run on the session's control worker ([`super::control`]).

use super::ap2::ptp::PtpPeer;
use super::control::{self, Op, Queue};
use super::discovery::Device;
use super::negotiate::{self, Local, Negotiated};
use super::rtsp::Rtsp;
use super::transport::{self, AudioOut, Shared, Timing};
use super::{LATENCY, LEAD, RATE};
use crate::output::Sink;
use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::aead::KeyInit;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// Once streaming, replies should be quick; a dead connection must not hold up the
/// control worker (and the reconnect) for long.
const RUNTIME_REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// TEARDOWN is a courtesy: never wait long for it.
const TEARDOWN_TIMEOUT: Duration = Duration::from_millis(500);
/// How long to wait for the audio thread to park or drain.
const HANDOFF_TIMEOUT: Duration = Duration::from_millis(200);

pub struct AirPlaySink {
    /// AirPlay 2 reverse event channel; held open for the session.
    _events: Option<TcpStream>,
    /// Keeps our PTP grandmaster talking to this receiver.
    _ptp: Option<PtpPeer>,
    shared: Arc<Shared>,
    producer: rtrb::Producer<i16>,
    /// Requests for the control worker, and the worker.
    control: Arc<Queue>,
    worker: Option<JoinHandle<()>>,
    /// The RTSP connection, to cut a request short when the session is lost.
    rtsp_socket: TcpStream,
    threads: Vec<JoinHandle<()>>,
}

/// The control worker's side of a session: the RTSP connection and what it needs to
/// pause, flush and set the volume.
struct Session {
    rtsp: Rtsp,
    shared: Arc<Shared>,
    /// Last volume sent, in tenths of a dB, so repeats aren't sent again.
    volume_sent: Option<i32>,
    name: String,
    /// The Mac's address the session was set up from, and the speaker's.
    local_ip: IpAddr,
    remote_ip: IpAddr,
}

impl AirPlaySink {
    pub fn connect(device: &Device, volume: f32) -> Result<Self, String> {
        if !device.supported {
            return Err(format!("{} requires encrypted AirPlay (not supported)", device.name));
        }
        let stream = connect_any(device)?;
        let local_ip = stream.local_addr().map_err(|e| e.to_string())?.ip();
        let remote_ip = stream.peer_addr().map_err(|e| e.to_string())?.ip();
        let session_id = fastrand::u32(..);
        let user_agent = if device.ap2 { "AirPlay/381.13" } else { "iTunes/11.0.4 (Windows; N)" };
        super::dacp::advertise_on(local_ip);
        let mut rtsp = Rtsp::new(stream, format!("rtsp://{local_ip}/{session_id}"), user_agent, super::dacp::id().to_string())?;

        let any: IpAddr = if local_ip.is_ipv4() { [0, 0, 0, 0].into() } else { Ipv6Addr::UNSPECIFIED.into() };
        let bind = || UdpSocket::bind(SocketAddr::new(any, 0)).map_err(|e| e.to_string());
        let (control, timing, audio) = (bind()?, bind()?, bind()?);
        for s in [&control, &timing] {
            s.set_read_timeout(Some(Duration::from_millis(200))).ok();
        }
        let local = Local {
            ip: local_ip,
            session_id,
            control_port: control.local_addr().map_err(|e| e.to_string())?.port(),
            timing_port: timing.local_addr().map_err(|e| e.to_string())?.port(),
            seq: fastrand::u16(..),
            rtptime: fastrand::u32(..),
        };
        let shared = Arc::new(Shared::new(local.seq, local.rtptime));
        // Receivers sync clocks with us during SETUP, so answer timing requests first.
        let sh = shared.clone();
        let mut threads = vec![transport::spawn("airplay-timing", move || transport::timing_loop(timing, &sh))];

        let negotiated = if device.ap2 {
            negotiate::airplay2(&mut rtsp, device, &local, remote_ip)
        } else {
            negotiate::airplay1(&mut rtsp, device, &local, remote_ip)
        };
        let Negotiated { data_port, control_port, audio_key, events, ptp } = negotiated.map_err(|e| {
            shared.running.store(false, Ordering::Release);
            format!("{}: {e}", device.name)
        })?;

        audio.connect(SocketAddr::new(remote_ip, data_port)).map_err(|e| e.to_string())?;
        let control_dst = SocketAddr::new(remote_ip, control_port);
        // ~1 s of audio between the player and network pacing.
        let (producer, consumer) = rtrb::RingBuffer::new(RATE as usize * 2);
        let sh = shared.clone();
        let retransmits = control.try_clone().map_err(|e| e.to_string())?;
        threads.push(transport::spawn("airplay-control", move || transport::control_loop(retransmits, control_dst, &sh)));
        let out = AudioOut {
            socket: audio,
            control,
            control_dst,
            cipher: audio_key.map(|k| ChaCha20Poly1305::new(&k.into())),
            timing: ptp.as_ref().map_or(Timing::Ntp, |p| Timing::Ptp { clock_id: p.clock_id() }),
        };
        let sh = shared.clone();
        threads.push(transport::spawn("airplay-audio", move || transport::audio_loop(out, consumer, &sh)));

        rtsp.set_reply_timeout(RUNTIME_REPLY_TIMEOUT);
        log_info!(
            "airplay: connected to {} at {remote_ip} from {local_ip} ({}, {} timing, {}, data port {data_port}, seq={} rtptime={})",
            device.name,
            if device.ap2 { "AirPlay 2" } else { "AirPlay 1" },
            if ptp.is_some() { "PTP" } else { "NTP" },
            if audio_key.is_some() { "encrypted" } else { "plain" },
            local.seq,
            local.rtptime
        );
        let session = Session { rtsp, shared: shared.clone(), volume_sent: None, name: device.name.clone(), local_ip, remote_ip };
        let (control, worker, rtsp_socket) = start_control(session)?;
        let mut sink = Self { _events: events, _ptp: ptp, shared, producer, control, worker: Some(worker), rtsp_socket, threads };
        sink.set_volume(volume);
        Ok(sink)
    }
}

/// Start the control worker for `session`: its request queue, its thread, and a
/// handle to its RTSP connection.
fn start_control(session: Session) -> Result<(Arc<Queue>, JoinHandle<()>, TcpStream), String> {
    let socket = session.rtsp.socket().map_err(|e| e.to_string())?;
    let requests = Arc::new(Queue::default());
    let queue = requests.clone();
    let mut session = session;
    let worker = transport::spawn("airplay-rtsp", move || {
        control::run(&queue, |op| session.apply(op));
        session.teardown();
    });
    Ok((requests, worker, socket))
}

impl Session {
    fn apply(&mut self, op: Op) {
        match op {
            Op::Pause => self.halt(),
            Op::Resume => self.resume(),
            Op::Flush => self.flush(),
            Op::Volume(volume) => self.set_volume(volume),
        }
    }

    fn resume(&mut self) {
        self.moved();
        log_debug!(
            "airplay: resume at seq={} rtptime={}",
            self.shared.seq.load(Ordering::Acquire),
            self.shared.rtptime.load(Ordering::Acquire)
        );
        self.shared.restart.store(true, Ordering::Release);
        self.shared.playing.store(true, Ordering::Release);
    }

    /// Stop sending, have the receiver drop its buffer, throw away what is queued
    /// here, then carry on if it was playing. The sink takes no new audio until this
    /// is done (see [`Queue::flushing`]), so none of it is lost.
    fn flush(&mut self) {
        let was_playing = self.shared.playing.load(Ordering::Acquire);
        self.halt();
        self.shared.drain.store(true, Ordering::Release);
        wait_until(|| !self.shared.drain.load(Ordering::Acquire));
        if was_playing {
            self.resume();
        }
    }

    fn set_volume(&mut self, volume: f32) {
        let db = super::volume::to_db(volume);
        let tenths = (db * 10.0_f32).round() as i32;
        if self.volume_sent == Some(tenths) {
            return;
        }
        self.volume_sent = Some(tenths);
        log_debug!("airplay: volume {volume:.3} -> {db:.1} dB");
        let body = format!("volume: {db:.6}\r\n");
        let sent = self.rtsp.request("SET_PARAMETER", None, &[], Some(("text/parameters", body.as_bytes())));
        self.check(sent);
    }

    /// Say goodbye, only over a working connection and briefly.
    fn teardown(&mut self) {
        if !self.shared.broken.load(Ordering::Acquire) {
            self.rtsp.set_reply_timeout(TEARDOWN_TIMEOUT);
            let _ = self.rtsp.request("TEARDOWN", None, &[], None);
        }
    }

    /// Stop the audio thread and drop the receiver's buffer.
    fn halt(&mut self) {
        self.shared.playing.store(false, Ordering::Release);
        wait_until(|| self.shared.idle.load(Ordering::Acquire));
        if self.moved() {
            return;
        }
        let seq = self.shared.seq.load(Ordering::Acquire);
        let rtptime = self.shared.rtptime.load(Ordering::Acquire);
        let started = Instant::now();
        let flushed = self.rtsp.request("FLUSH", None, &[("RTP-Info", format!("seq={seq};rtptime={rtptime}"))], None);
        let ms = started.elapsed().as_millis();
        match &flushed {
            Ok(_) => log_debug!("airplay: FLUSH seq={seq} rtptime={rtptime} answered in {ms} ms"),
            Err(e) => log_debug!("airplay: FLUSH seq={seq} rtptime={rtptime} failed after {ms} ms: {e}"),
        }
        self.check(flushed);
    }

    /// Whether the Mac now reaches the speaker from another address than the session
    /// was set up from (e.g. Ethernet dropped on screen lock, haze reconnected over
    /// Wi-Fi, then Ethernet came back). The receiver only follows clock and sync
    /// packets from the session's address, so a stream that starts again (new track,
    /// seek, resume) would stay silent; mark the session lost so it is set up again.
    fn moved(&self) -> bool {
        let Some(now) = route_from(self.remote_ip) else { return false };
        if now == self.local_ip {
            return false;
        }
        if !self.shared.broken.swap(true, Ordering::AcqRel) {
            log_warn!("airplay: {} is now reached from {now}, not {}; reconnecting", self.name, self.local_ip);
        }
        true
    }

    /// A failed control request means the connection is gone.
    fn check<T>(&self, result: Result<T, String>) {
        if let Err(e) = result
            && !self.shared.broken.swap(true, Ordering::AcqRel)
        {
            log_warn!("airplay control failed, session lost: {e}");
        }
    }
}

/// Try each advertised address until one accepts (hosts list VPN/bridge addresses too).
fn connect_any(device: &Device) -> Result<TcpStream, String> {
    let mut last_err = String::from("no address");
    let stream = device
        .addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).map_err(|e| last_err = e.to_string()).ok())
        .ok_or_else(|| format!("{}: {last_err}", device.name))?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok();
    stream.set_nodelay(true).ok();
    Ok(stream)
}

/// The local address the OS would send from to reach `remote` (nothing is sent).
fn route_from(remote: IpAddr) -> Option<IpAddr> {
    let any: IpAddr = if remote.is_ipv4() { [0, 0, 0, 0].into() } else { Ipv6Addr::UNSPECIFIED.into() };
    let probe = UdpSocket::bind(SocketAddr::new(any, 0)).ok()?;
    probe.connect(SocketAddr::new(remote, 9)).ok()?;
    Some(probe.local_addr().ok()?.ip())
}

fn wait_until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + HANDOFF_TIMEOUT;
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl Sink for AirPlaySink {
    fn write(&mut self, samples: &[i16]) -> usize {
        // While a flush waits for the worker, new audio would be thrown away with
        // the old (like the local sink's).
        if self.control.flushing() || self.producer.slots() < samples.len() {
            return 0;
        }
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples.len()) {
            chunk.fill_from_iter(samples.iter().copied());
        }
        samples.len()
    }

    fn pending_frames(&self) -> usize {
        // What a pending flush will drop doesn't count.
        if self.control.flushing() {
            return 0;
        }
        (self.producer.buffer().capacity() - self.producer.slots()) / 2
    }

    fn latency_frames(&self) -> usize {
        LATENCY as usize + LEAD as usize
    }

    fn drops_buffer_on_pause(&self) -> bool {
        true
    }

    fn pause(&mut self) {
        self.control.push(Op::Pause);
    }

    fn resume(&mut self) {
        self.control.push(Op::Resume);
    }

    fn flush(&mut self) {
        self.control.push(Op::Flush);
    }

    fn set_volume(&mut self, volume: f32) {
        self.control.push(Op::Volume(volume));
    }

    fn healthy(&self) -> bool {
        !self.shared.broken.load(Ordering::Acquire)
    }
}

impl Drop for AirPlaySink {
    fn drop(&mut self) {
        self.shared.playing.store(false, Ordering::Release);
        // The worker drops what is still queued, says goodbye (if the connection
        // works) and ends. A lost session's request in flight is cut short rather
        // than left to time out.
        self.control.close();
        if !self.healthy() {
            let _ = self.rtsp_socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.shared.running.store(false, Ordering::Release);
        // Ends the event channel reader, which holds its own handle to the socket.
        if let Some(events) = &self._events {
            let _ = events.shutdown(std::net::Shutdown::Both);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        log_debug!("airplay: session ended: {}", self.shared.stats.summary());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// A speaker's RTSP side on localhost: answers every request with 200 (FLUSH
    /// after `flush_delay`), or hangs up on `fail_on`. Records "METHOD body".
    fn fake_speaker(flush_delay: Duration, fail_on: Option<&'static str>) -> (TcpStream, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let method = line.split_whitespace().next().unwrap_or("").to_string();
                let (mut cseq, mut length) = (String::new(), 0);
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let l = line.trim_end();
                    if l.is_empty() {
                        break;
                    }
                    if let Some(v) = l.strip_prefix("CSeq: ") {
                        cseq = v.to_string();
                    }
                    if let Some(v) = l.strip_prefix("Content-Length: ") {
                        length = v.parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(format!("{method} {}", String::from_utf8_lossy(&body).trim_end()).trim_end().to_string());
                if fail_on == Some(method.as_str()) {
                    return; // hang up
                }
                if method == "FLUSH" {
                    std::thread::sleep(flush_delay);
                }
                if writer.write_all(format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n\r\n").as_bytes()).is_err() {
                    return;
                }
            }
        });
        (TcpStream::connect(addr).unwrap(), seen)
    }

    /// A sink streaming to localhost, controlled through `rtsp` (no handshake).
    fn sink_on(rtsp: TcpStream) -> AirPlaySink {
        let local_ip: IpAddr = [127, 0, 0, 1].into();
        let shared = Arc::new(Shared::new(1, 1));
        // Somewhere for the audio to go, kept open so sends don't fail.
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let audio = UdpSocket::bind("127.0.0.1:0").unwrap();
        audio.connect(receiver.local_addr().unwrap()).unwrap();
        let out = AudioOut {
            socket: audio,
            control: UdpSocket::bind("127.0.0.1:0").unwrap(),
            control_dst: receiver.local_addr().unwrap(),
            cipher: None,
            timing: Timing::Ntp,
        };
        let (producer, consumer) = rtrb::RingBuffer::new(RATE as usize * 2);
        let sh = shared.clone();
        let audio_thread = transport::spawn("airplay-audio", move || {
            let _receiver = receiver;
            transport::audio_loop(out, consumer, &sh)
        });
        let rtsp = Rtsp::new(rtsp, "rtsp://127.0.0.1/1".into(), "test", "0".into()).unwrap();
        rtsp.set_reply_timeout(RUNTIME_REPLY_TIMEOUT);
        let session = Session { rtsp, shared: shared.clone(), volume_sent: None, name: "test".into(), local_ip, remote_ip: local_ip };
        let (control, worker, rtsp_socket) = start_control(session).unwrap();
        AirPlaySink { _events: None, _ptp: None, shared, producer, control, worker: Some(worker), rtsp_socket, threads: vec![audio_thread] }
    }

    /// Wait up to 3 s for `done`.
    fn eventually(mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        true
    }

    #[test]
    fn control_requests_wait_for_no_reply_and_keep_their_order() {
        let (rtsp, seen) = fake_speaker(Duration::from_millis(300), None);
        let mut sink = sink_on(rtsp);
        sink.set_volume(0.5);
        assert_eq!(sink.write(&[1; 704]), 704);

        let asked = Instant::now();
        sink.flush();
        // A fade's worth of volume changes while the speaker takes its time.
        for v in [0.4, 0.3, 0.2] {
            sink.set_volume(v);
        }
        assert!(asked.elapsed() < Duration::from_millis(100), "returned at once: {:?}", asked.elapsed());
        // Until the flush is done, audio would be thrown away with the old.
        assert_eq!((sink.write(&[1; 704]), sink.pending_frames()), (0, 0));
        assert!(eventually(|| sink.write(&[2; 704]) == 704));
        assert!(asked.elapsed() >= Duration::from_millis(300), "accepted only after the FLUSH reply");
        assert!(sink.healthy());

        assert!(eventually(|| seen.lock().unwrap().len() == 3));
        drop(sink);
        assert_eq!(
            *seen.lock().unwrap(),
            ["SET_PARAMETER volume: -15.000000", "FLUSH", "SET_PARAMETER volume: -24.000000", "TEARDOWN"],
            "in order, the volumes during the flush sent as one"
        );
    }

    #[test]
    fn a_failed_control_request_marks_the_session_lost() {
        let (rtsp, _seen) = fake_speaker(Duration::ZERO, Some("SET_PARAMETER"));
        let mut sink = sink_on(rtsp);
        let asked = Instant::now();
        sink.set_volume(0.7);
        assert!(asked.elapsed() < Duration::from_millis(100));
        assert!(eventually(|| !sink.healthy()), "the player reconnects");
        let dropped = Instant::now();
        drop(sink);
        assert!(dropped.elapsed() < Duration::from_secs(1), "no goodbye over a lost session: {:?}", dropped.elapsed());
    }

    #[test]
    fn route_from_names_the_sending_address() {
        assert_eq!(route_from([127, 0, 0, 1].into()), Some([127, 0, 0, 1].into()));
    }
}
