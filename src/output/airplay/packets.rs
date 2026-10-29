//! AirPlay wire formats: ALAC frames, RTP audio packets, sync, timing and
//! retransmit packets.

use super::ap2::{pairing, ptp};
use super::{FRAMES_PER_PACKET, LATENCY, MIN_LATENCY};
use chacha20poly1305::ChaCha20Poly1305;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds between 1900 (NTP epoch) and 1970 (Unix epoch).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// Current time as a 64-bit NTP timestamp (32.32 fixed point).
pub fn ntp_now() -> u64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    ((d.as_secs() + NTP_UNIX_OFFSET) << 32) | (((d.subsec_nanos() as u64) << 32) / 1_000_000_000)
}

struct BitWriter {
    buf: Vec<u8>,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.bits.is_multiple_of(8) {
                self.buf.push(0);
            }
            if (value >> i) & 1 == 1 {
                *self.buf.last_mut().unwrap() |= 0x80 >> (self.bits % 8);
            }
            self.bits += 1;
        }
    }
}

/// One ALAC frame using the "escape" (uncompressed) path: CPE header, raw 16-bit
/// interleaved samples, END tag. Every ALAC decoder accepts this.
pub fn alac_frame(samples: &[i16]) -> Vec<u8> {
    debug_assert_eq!(samples.len(), FRAMES_PER_PACKET * 2);
    let mut w = BitWriter { buf: Vec::with_capacity(FRAMES_PER_PACKET * 4 + 4), bits: 0 };
    w.put(1, 3); // ID_CPE (stereo pair)
    w.put(0, 4); // element instance
    w.put(0, 12); // unused
    w.put(0, 1); // partial frame: no, always 352 frames
    w.put(0, 2); // bytes shifted
    w.put(1, 1); // escape flag: uncompressed
    for &s in samples {
        w.put(s as u16 as u32, 16);
    }
    w.put(7, 3); // ID_END
    w.buf
}

/// RTP audio packet; payload sealed with the AirPlay 2 audio key when given.
pub fn audio_packet(seq: u16, rtptime: u32, ssrc: u32, first: bool, samples: &[i16], cipher: Option<&ChaCha20Poly1305>) -> Vec<u8> {
    let mut header = [0u8; 12];
    header[0] = 0x80;
    header[1] = if first { 0xe0 } else { 0x60 }; // payload type 96, marker on the first packet
    header[2..4].copy_from_slice(&seq.to_be_bytes());
    header[4..8].copy_from_slice(&rtptime.to_be_bytes());
    header[8..12].copy_from_slice(&ssrc.to_be_bytes());
    let payload = alac_frame(samples);
    match cipher {
        Some(c) => pairing::seal_audio(c, &header, &payload, seq),
        None => [&header[..], &payload].concat(),
    }
}

/// NTP-timed sync: the frame playing now, the time, and the frame being sent now.
pub fn sync_ntp(now_rtp: u32, first: bool) -> [u8; 20] {
    let mut p = [0u8; 20];
    p[..4].copy_from_slice(&[if first { 0x90 } else { 0x80 }, 0xd4, 0x00, 0x07]);
    p[4..8].copy_from_slice(&now_rtp.wrapping_sub(LATENCY).to_be_bytes());
    p[8..16].copy_from_slice(&ntp_now().to_be_bytes());
    p[16..20].copy_from_slice(&now_rtp.to_be_bytes());
    p
}

/// PTP-timed sync (AirPlay 2): frame playing now, PTP time, stream anchor, clock id.
pub fn sync_ptp(now_rtp: u32, head_rtp: u32, clock_id: u64, first: bool) -> [u8; 28] {
    let mut p = [0u8; 28];
    p[..4].copy_from_slice(&[if first { 0x90 } else { 0x80 }, 0xd7, 0x00, 0x06]);
    p[4..8].copy_from_slice(&now_rtp.wrapping_sub(LATENCY).to_be_bytes());
    p[8..16].copy_from_slice(&ptp::now_ns().to_be_bytes());
    p[16..20].copy_from_slice(&head_rtp.wrapping_sub(MIN_LATENCY).to_be_bytes());
    p[20..28].copy_from_slice(&clock_id.to_be_bytes());
    p
}

/// Reply to an NTP timing request, or None if `request` isn't one.
pub fn timing_reply(request: &[u8]) -> Option<[u8; 32]> {
    if request.len() < 32 || request[1] & 0x7f != 0x52 {
        return None;
    }
    let now = ntp_now().to_be_bytes();
    let mut reply = [0u8; 32];
    reply[..4].copy_from_slice(&[0x80, 0xd3, 0x00, 0x07]);
    reply[8..16].copy_from_slice(&request[24..32]); // their transmit time -> our origin
    reply[16..24].copy_from_slice(&now);
    reply[24..32].copy_from_slice(&now);
    Some(reply)
}

/// (first missing sequence number, count) from a retransmit request.
pub fn retransmit_request(packet: &[u8]) -> Option<(u16, u16)> {
    // 0x80 0xd5 <seq> <first missing> <count>
    if packet.len() < 8 || packet[1] & 0x7f != 0x55 {
        return None;
    }
    Some((u16::from_be_bytes([packet[4], packet[5]]), u16::from_be_bytes([packet[6], packet[7]])))
}

/// A previously sent packet, wrapped for retransmission.
pub fn retransmission(seq: u16, original: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(original.len() + 4);
    out.extend_from_slice(&[0x80, 0xd6]);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(original);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alac_layout() {
        let mut s = vec![0i16; FRAMES_PER_PACKET * 2];
        s[0] = 0x1234;
        s[1] = -1;
        let f = alac_frame(&s);
        // 23 header bits + 352*32 sample bits + 3 end bits = 11290 bits -> 1412 bytes
        assert_eq!(f.len(), 1412);
        // header: 001 0000 000000000000 0 00 1 -> first 23 bits
        assert_eq!(f[0], 0b0010_0000);
        assert_eq!(f[1], 0);
        assert_eq!(f[2] & 0b1111_1110, 0b0000_0010);
        // first sample 0x1234 starts at bit 23
        let bits = |from: usize, n: usize| (from..from + n).fold(0u32, |acc, b| (acc << 1) | ((f[b / 8] >> (7 - b % 8)) & 1) as u32);
        assert_eq!(bits(23, 16), 0x1234);
        assert_eq!(bits(39, 16), 0xffff);
        assert_eq!(bits(23 + 352 * 32, 3), 7);
    }

    /// Decode our escape frames with a real ALAC decoder.
    #[test]
    fn alac_decodes() {
        use symphonia::core::codecs::audio::well_known::CODEC_ID_ALAC;
        use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
        use symphonia::core::packet::Packet;
        // ALACSpecificConfig matching our SDP fmtp line.
        let mut cookie = Vec::new();
        cookie.extend(352u32.to_be_bytes());
        cookie.extend([0, 16, 40, 10, 14, 2]);
        cookie.extend(255u16.to_be_bytes());
        cookie.extend(0u32.to_be_bytes());
        cookie.extend(0u32.to_be_bytes());
        cookie.extend(44100u32.to_be_bytes());
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_ALAC).with_extra_data(cookie.into_boxed_slice()).with_sample_rate(44100);
        let mut dec = symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default()).unwrap();
        let input: Vec<i16> = (0..FRAMES_PER_PACKET * 2).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
        let pkt = Packet::new(0, 0.into(), 352u64.into(), alac_frame(&input));
        let mut out = Vec::<i16>::new();
        dec.decode(&pkt).unwrap().copy_to_vec_interleaved(&mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn control_packets() {
        let mut req = [0u8; 32];
        req[1] = 0xd2;
        req[24..32].copy_from_slice(&[9; 8]);
        let reply = timing_reply(&req).unwrap();
        assert_eq!((reply[1], &reply[8..16]), (0xd3, &[9u8; 8][..]));
        assert!(timing_reply(&req[..20]).is_none());
        assert_eq!(retransmit_request(&[0x80, 0xd5, 0, 1, 0, 5, 0, 3]), Some((5, 3)));
        assert_eq!(retransmission(7, &[1, 2])[..], [0x80, 0xd6, 0, 7, 1, 2]);
    }
}
