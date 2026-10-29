//! Channel logos and posters: fetched and decoded off the UI thread, shrunk to the
//! size they're shown at, uploaded as textures and evicted least-recently-used once
//! a byte budget is exceeded.

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Decoded RGBA kept on the GPU; ~16 MB is thousands of logos.
const BUDGET_BYTES: usize = 16 << 20;
/// Pending fetches beyond this are dropped (oldest first) and re-requested if needed.
const MAX_QUEUE: usize = 48;
const WORKERS: usize = 3;
/// Logos are shrunk to at most this many pixels across (providers send huge ones).
const MAX_SIDE: usize = 160;

enum Slot {
    Loading,
    Failed,
    Ready { tex: TextureHandle, bytes: usize, last_used: u64 },
}

enum Done {
    Image(String, ColorImage),
    Failed(String),
}

type Queue = Arc<(Mutex<VecDeque<String>>, Condvar)>;

pub struct Logos {
    ctx: egui::Context,
    slots: HashMap<String, Slot>,
    queue: Queue,
    done: Receiver<Done>,
    /// For the workers, until they start (with the first cover asked for).
    done_tx: Option<Sender<Done>>,
    frame: u64,
    bytes: usize,
}

impl Logos {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let (done_tx, done) = mpsc::channel();
        Self { ctx: ctx.clone(), slots: HashMap::new(), queue, done, done_tx: Some(done_tx), frame: 0, bytes: 0 }
    }

    /// Start the fetching threads, once.
    fn start_workers(&mut self) {
        let Some(tx) = self.done_tx.take() else { return };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .user_agent(crate::iptv::http::USER_AGENT)
            .build()
            .into();
        for i in 0..WORKERS {
            let (queue, tx, agent, ctx) = (self.queue.clone(), tx.clone(), agent.clone(), self.ctx.clone());
            std::thread::Builder::new().name(format!("logos-{i}")).spawn(move || worker(queue, tx, agent, ctx)).expect("spawn logo worker");
        }
    }

    /// Call once per frame before drawing.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        while let Ok(d) = self.done.try_recv() {
            match d {
                Done::Image(url, img) => {
                    let bytes = img.pixels.len() * 4;
                    let tex = self.ctx.load_texture(&url, img, TextureOptions::LINEAR);
                    self.bytes += bytes;
                    self.slots.insert(url, Slot::Ready { tex, bytes, last_used: self.frame });
                }
                Done::Failed(url) => {
                    self.slots.insert(url, Slot::Failed);
                }
            }
        }
        if self.bytes > BUDGET_BYTES {
            self.evict();
        }
    }

    fn evict(&mut self) {
        let mut ready: Vec<(u64, String, usize)> = self
            .slots
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready { last_used, bytes, .. } if *last_used + 2 < self.frame => Some((*last_used, k.clone(), *bytes)),
                _ => None,
            })
            .collect();
        ready.sort_unstable();
        for (_, url, bytes) in ready {
            if self.bytes <= BUDGET_BYTES * 3 / 4 {
                break;
            }
            self.slots.remove(&url);
            self.bytes -= bytes;
        }
    }

    /// Texture for `url` if loaded; otherwise starts loading it and returns None.
    /// Texture for `url` and its size if loaded; otherwise starts loading it and
    /// returns None.
    pub fn get(&mut self, url: &str) -> Option<(TextureId, egui::Vec2)> {
        match self.slots.get_mut(url) {
            Some(Slot::Ready { tex, last_used, .. }) => {
                *last_used = self.frame;
                Some((tex.id(), tex.size_vec2()))
            }
            Some(_) => None,
            None => {
                self.start_workers();
                self.slots.insert(url.to_string(), Slot::Loading);
                let (lock, cvar) = &*self.queue;
                let mut q = lock.lock().unwrap();
                q.push_front(url.to_string());
                // Drop the stalest requests; they are re-requested if still visible.
                while q.len() > MAX_QUEUE {
                    if let Some(old) = q.pop_back() {
                        self.slots.remove(&old);
                    }
                }
                cvar.notify_one();
                None
            }
        }
    }
}

fn worker(queue: Queue, tx: Sender<Done>, agent: ureq::Agent, ctx: egui::Context) {
    loop {
        let url = {
            let (lock, cvar) = &*queue;
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(u) = q.pop_front() {
                    break u;
                }
                q = cvar.wait(q).unwrap();
            }
        };
        let result = fetch(&agent, &url).map(|img| Done::Image(url.clone(), img)).unwrap_or(Done::Failed(url));
        if tx.send(result).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn fetch(agent: &ureq::Agent, url: &str) -> Option<ColorImage> {
    if url.is_empty() {
        return None;
    }
    let bytes = agent.get(url).call().ok()?.into_body().with_config().limit(8 << 20).read_to_vec().ok()?;
    decode(&bytes).map(shrink)
}

/// JPEG, PNG or WebP, by their signature.
fn decode(bytes: &[u8]) -> Option<ColorImage> {
    if bytes.starts_with(b"\x89PNG") {
        return decode_png(bytes);
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return decode_webp(bytes);
    }
    decode_jpeg(bytes)
}

fn decode_jpeg(bytes: &[u8]) -> Option<ColorImage> {
    let mut dec =
        JpegDecoder::new_with_options(std::io::Cursor::new(bytes), DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA));
    let pixels = dec.decode().ok()?;
    let info = dec.info()?;
    image_from(&pixels, [info.width as usize, info.height as usize])
}

fn decode_png(bytes: &[u8]) -> Option<ColorImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // 8-bit samples, palettes expanded.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(frame.buffer_size());
    let size = [frame.width as usize, frame.height as usize];
    match frame.color_type {
        // Grey with alpha: logos are often white on transparent.
        png::ColorType::GrayscaleAlpha => {
            image_from(&pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect::<Vec<_>>(), size)
        }
        _ => image_from(&pixels, size),
    }
}

fn decode_webp(bytes: &[u8]) -> Option<ColorImage> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let (width, height) = decoder.dimensions();
    let mut pixels = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut pixels).ok()?;
    image_from(&pixels, [width as usize, height as usize])
}

/// `img` scaled down (area average) so neither side exceeds [`MAX_SIDE`].
fn shrink(img: ColorImage) -> ColorImage {
    let [w, h] = img.size;
    let k = w.max(h).div_ceil(MAX_SIDE);
    if k <= 1 {
        return img;
    }
    let (nw, nh) = (w / k, h / k);
    let mut pixels = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let c = img.pixels[(y * k + dy) * w + x * k + dx];
                    for (a, v) in acc.iter_mut().zip(c.to_array()) {
                        *a += u32::from(v);
                    }
                }
            }
            let n = (k * k) as u32;
            pixels.push(egui::Color32::from_rgba_premultiplied(
                (acc[0] / n) as u8,
                (acc[1] / n) as u8,
                (acc[2] / n) as u8,
                (acc[3] / n) as u8,
            ));
        }
    }
    ColorImage::new([nw, nh], pixels)
}

/// An image from 1 (grey), 3 (RGB) or 4 (RGBA) bytes per pixel.
fn image_from(pixels: &[u8], size: [usize; 2]) -> Option<ColorImage> {
    let area = size[0] * size[1];
    if area == 0 || !pixels.len().is_multiple_of(area) {
        return None;
    }
    match pixels.len() / area {
        1 => Some(ColorImage::from_gray(size, pixels)),
        3 => Some(ColorImage::from_rgb(size, pixels)),
        4 => Some(ColorImage::from_rgba_unmultiplied(size, pixels)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×1 RGB PNG: one red, one green pixel.
    fn png_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&[255, 0, 0, 0, 255, 0]).unwrap();
        out
    }

    #[test]
    fn decodes_png() {
        let image = decode(&png_bytes()).unwrap();
        assert_eq!(image.size, [2, 1]);
        assert_eq!(image.pixels[0], egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(image.pixels[1], egui::Color32::from_rgb(0, 255, 0));
    }

    #[test]
    fn shrinks_big_logos() {
        let img = ColorImage::new([1000, 500], vec![egui::Color32::WHITE; 500_000]);
        let small = shrink(img);
        assert_eq!(small.size, [142, 71]);
        assert_eq!(small.pixels[0], egui::Color32::WHITE);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not an image").is_none());
        assert!(decode(b"RIFF\0\0\0\0WEBPVP8 ").is_none());
    }
}
