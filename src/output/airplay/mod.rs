//! AirPlay sender: RTSP control, RTP/UDP audio as uncompressed ALAC, timing,
//! sync packets and retransmits.
//!
//! Two handshakes share that streaming core:
//! - AirPlay 2 (devices with CoreUtils pairing, e.g. Sonos Era, HomePod): transient
//!   HomeKit pairing, encrypted RTSP, binary-plist SETUP, PTP timing,
//!   ChaCha20-Poly1305 audio.
//! - AirPlay 1 / RAOP with unencrypted audio (`et=0`): AirPort Express, older
//!   speakers, shairport-sync classic.
//!
//! - [`discovery`]: finding speakers over mDNS
//! - [`sink`]: [`AirPlaySink`], the audio output
//! - [`control`]: the worker that sends a session's pause, flush and volume requests
//! - [`negotiate`]: the AirPlay 1 / AirPlay 2 session handshakes
//! - [`rtsp`]: the (optionally encrypted) RTSP client
//! - [`packets`]: wire formats (ALAC frames, RTP, sync and timing packets)
//! - [`transport`]: the network threads (audio pacing, timing, retransmits)
//! - [`ap2`]: AirPlay 2 building blocks (pairing, plists, PTP clock)
//! - [`dacp`]: remote control from the speaker's buttons
//! - [`volume`]: AirPlay's dB volume scale, both ways

pub mod ap2;
mod control;
pub mod dacp;
mod discovery;
mod negotiate;
mod packets;
mod rtsp;
mod sink;
mod transport;
pub mod volume;

pub use discovery::{Device, Discovery};
pub use sink::AirPlaySink;

use super::RATE;

/// ALAC frames per RTP packet.
const FRAMES_PER_PACKET: usize = 352;
/// Receiver-side buffer we ask for (2 s), in frames.
const LATENCY: u32 = 88_200;
/// Minimum latency advertised to AirPlay 2 receivers (0.25 s), in frames.
const MIN_LATENCY: u32 = 11_025;
/// How far ahead of real time we push audio, in frames.
const LEAD: u64 = RATE as u64 / 10;
/// Sent packets kept for retransmission.
const HISTORY: usize = 1024;
