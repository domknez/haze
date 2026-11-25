//! Recording what plays: the packets of the picture and the sound go, as they are
//! read, into a Matroska file as well as to the decoders. Nothing is decoded or
//! encoded again, and the provider sees no second connection.

use ffmpeg::Rescale;
use ffmpeg_next as ffmpeg;
use std::path::Path;

/// Microseconds, the unit the [`Timeline`] counts in.
const MICROS: ffmpeg::Rational = ffmpeg::Rational(1, 1_000_000);

/// A recording being written.
pub struct Recorder {
    output: ffmpeg::format::context::Output,
    /// For each stream of the input: the stream it is in the file, and the input's
    /// time base, if it's recorded.
    streams: Vec<Option<(usize, ffmpeg::Rational)>>,
    /// The picture, whose key frame the file starts at (with no picture, the file
    /// starts at once).
    video: Option<usize>,
    timeline: Timeline,
}

impl Recorder {
    /// Start a file at `path` with the input's streams `recorded`; `video` is the
    /// picture among them, if there is one.
    pub fn start(
        path: &Path,
        input: &ffmpeg::format::context::Input,
        recorded: &[usize],
        video: Option<usize>,
    ) -> Result<Recorder, ffmpeg::Error> {
        let mut output = ffmpeg::format::output_as(path, "matroska")?;
        let mut streams = vec![None; input.nb_streams() as usize];
        for &i in recorded {
            let Some(stream) = input.stream(i) else { continue };
            let mut out = output.add_stream(None::<ffmpeg::Codec>)?;
            out.set_parameters(stream.parameters());
            out.set_metadata(stream.metadata().to_owned());
            // SAFETY: the parameters were just copied into this stream of the file. The
            // tag names the codec as the input's container did; the file names its own.
            unsafe { (*out.parameters().as_mut_ptr()).codec_tag = 0 };
            if let Some(slot) = streams.get_mut(i) {
                *slot = Some((out.index(), stream.time_base()));
            }
        }
        output.write_header()?;
        let timeline = Timeline::new(output.nb_streams() as usize);
        Ok(Recorder { output, streams, video, timeline })
    }

    /// Write `packet` to the file, if its stream is recorded and it has a place in it.
    pub fn write(&mut self, packet: &ffmpeg::Packet) -> Result<(), ffmpeg::Error> {
        let index = packet.stream();
        let Some(&Some((out, time_base))) = self.streams.get(index) else { return Ok(()) };
        let Some(t) = packet.dts().or(packet.pts()) else { return Ok(()) };
        let opens = self.video.is_none_or(|v| v == index && packet.is_key());
        let Some(shift) = self.timeline.place(out, t.rescale(time_base, MICROS), opens) else { return Ok(()) };
        let Some(file_base) = self.output.stream(out).map(|s| s.time_base()) else { return Ok(()) };
        let mut packet = packet.clone();
        packet.rescale_ts(time_base, file_base);
        let shift = shift.rescale(MICROS, file_base);
        packet.set_pts(packet.pts().map(|t| t - shift));
        packet.set_dts(packet.dts().map(|t| t - shift));
        packet.set_stream(out);
        packet.set_position(-1);
        packet.write_interleaved(&mut self.output)
    }
}

impl Drop for Recorder {
    /// Close the file properly: its index, so players can seek in it.
    fn drop(&mut self) {
        if let Err(e) = self.output.write_trailer() {
            log_warn!("recording: couldn't finish the file: {e}");
        }
    }
}

/// How far the stream's clock may jump between packets before it counts as a new
/// start (a provider restarting the stream), in microseconds.
const JUMP: i64 = 10_000_000;

/// Where packets go in the recording, which starts at 0: the stream's own clock
/// starts anywhere and may jump back or ahead (the stream restarted), while the
/// file's runs on.
#[derive(Debug)]
struct Timeline {
    /// The stream time the recording starts at.
    start: Option<i64>,
    /// For each stream of the file: what's taken off its times.
    shift: Vec<Option<i64>>,
    /// For each stream of the file: where its last packet went.
    last: Vec<Option<i64>>,
    /// Where the latest packet of any stream went.
    latest: i64,
}

impl Timeline {
    fn new(streams: usize) -> Timeline {
        Timeline { start: None, shift: vec![None; streams], last: vec![None; streams], latest: 0 }
    }

    /// What to take off the time `t` of a packet of `stream` (both in microseconds),
    /// or None to leave the packet out: before the recording `opens` (on a key frame
    /// of the picture), before its start, or behind its stream's last packet.
    fn place(&mut self, stream: usize, t: i64, opens: bool) -> Option<i64> {
        let start = match self.start {
            Some(start) => start,
            None if opens => *self.start.insert(t),
            None => return None,
        };
        let shift = self.shift.get_mut(stream)?;
        let mut at = t - *shift.get_or_insert(start);
        if (at - self.latest).abs() > JUMP {
            // The stream's clock jumped: carry on from where the file is.
            *shift = Some(t - self.latest);
            at = self.latest;
        }
        let last = self.last.get_mut(stream)?;
        if at < 0 || last.is_some_and(|l| at < l) {
            return None;
        }
        *last = Some(at);
        self.latest = self.latest.max(at);
        *shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000;

    #[test]
    fn starts_at_zero_on_a_key_frame() {
        let mut t = Timeline::new(2);
        assert_eq!(t.place(1, 99 * S, false), None, "sound before the first key frame");
        assert_eq!(t.place(0, 100 * S, true), Some(100 * S));
        assert_eq!(t.place(1, 100 * S + 20_000, false), Some(100 * S));
        assert_eq!(t.place(1, 99 * S, false), None, "sound from before the start");
    }

    #[test]
    fn leaves_out_what_goes_back() {
        let mut t = Timeline::new(1);
        t.place(0, 10 * S, true);
        t.place(0, 11 * S, false);
        assert_eq!(t.place(0, 10 * S + 500_000, false), None);
        assert_eq!(t.place(0, 11 * S, false), Some(10 * S), "the same time again is fine");
    }

    #[test]
    fn carries_on_when_the_clock_jumps() {
        let mut t = Timeline::new(2);
        t.place(0, 1000 * S, true);
        t.place(1, 1000 * S, false);
        t.place(0, 1005 * S, false);
        // The stream restarted with its clock at 3 s: the picture carries on at 5 s.
        assert_eq!(t.place(0, 3 * S, false), Some(3 * S - 5 * S));
        // The sound jumps too, a moment later, and lands beside the picture.
        assert_eq!(t.place(1, 3 * S + 10_000, false), Some(3 * S + 10_000 - 5 * S));
        // And ahead: an hour later on the stream's clock, a moment later in the file.
        assert_eq!(t.place(0, 3603 * S, false), Some(3603 * S - 5 * S));
    }
}
