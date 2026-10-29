//! Decoded pictures as the screen takes them: Y, U and V planes (or Y and
//! interleaved UV), converted to colour on the GPU. That's 1.5 bytes a pixel to copy
//! and upload instead of 4, and no colour conversion on the CPU.

use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::util::frame::Video;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Three planes: Y at full size, U and V at half width and height.
    I420,
    /// Y, then U and V interleaved at half size (what hardware decoders give).
    Nv12,
}

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    /// Tightly packed planes (no row padding).
    pub planes: Vec<Vec<u8>>,
    /// BT.709 colours (HD) rather than BT.601 (SD).
    pub bt709: bool,
    /// Full 0–255 range rather than TV range 16–235.
    pub full_range: bool,
    /// Width / height as shown (pixels aren't always square).
    pub aspect: f32,
    /// Presentation time, seconds.
    pub pts: f64,
}

impl VideoFrame {
    /// Copies a decoded frame in a layout the screen takes as it is; none for other
    /// pixel formats (convert those to I420 first).
    pub fn from_decoded(f: &Video, pts: f64) -> Option<VideoFrame> {
        let (layout, full_range) = match f.format() {
            Pixel::YUV420P => (Layout::I420, f.color_range() == ffmpeg::color::Range::JPEG),
            Pixel::YUVJ420P => (Layout::I420, true),
            Pixel::NV12 => (Layout::Nv12, f.color_range() == ffmpeg::color::Range::JPEG),
            _ => return None,
        };
        let (w, h) = (f.width() as usize, f.height() as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let plane = |i: usize, row_bytes: usize, rows: usize| {
            let (data, stride) = (f.data(i), f.stride(i));
            let mut out = Vec::with_capacity(row_bytes * rows);
            for r in 0..rows {
                out.extend_from_slice(&data[r * stride..r * stride + row_bytes]);
            }
            out
        };
        let planes = match layout {
            Layout::I420 => vec![plane(0, w, h), plane(1, cw, ch), plane(2, cw, ch)],
            Layout::Nv12 => vec![plane(0, w, h), plane(1, cw * 2, ch)],
        };
        let sar = f.aspect_ratio();
        let sar = if sar.numerator() > 0 && sar.denominator() > 0 { sar.numerator() as f32 / sar.denominator() as f32 } else { 1.0 };
        let bt709 = match f.color_space() {
            ffmpeg::color::Space::BT709 => true,
            ffmpeg::color::Space::BT470BG | ffmpeg::color::Space::SMPTE170M => false,
            _ => h >= 720,
        };
        Some(VideoFrame {
            width: w as u32,
            height: h as u32,
            layout,
            planes,
            bt709,
            full_range,
            aspect: w as f32 * sar / h.max(1) as f32,
            pts,
        })
    }
}
