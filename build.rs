//! Decodes the app icon PNGs to raw RGBA at build time (so the binary needs no PNG
//! decoder at runtime) and, on Windows, embeds haze.ico into the executable.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// macOS icons leave a transparent margin around the artwork; without it the Dock /
/// Cmd-Tab icon looks larger than every other app's. Apple's template says 824/1024;
/// measured against current Dock icons, ~836/1024 lines up exactly.
const MACOS_ART_FRACTION: f32 = 836.0 / 1024.0;

fn decode(src: &str) -> (Vec<u8>, usize) {
    println!("cargo:rerun-if-changed={src}");
    let decoder = png::Decoder::new(BufReader::new(File::open(src).unwrap_or_else(|e| panic!("{src}: {e}"))));
    let mut reader = decoder.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png data");
    assert_eq!(info.color_type, png::ColorType::Rgba, "{src}: expected RGBA");
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "{src}: expected 8-bit");
    assert_eq!(info.width, info.height, "{src}: expected square");
    buf.truncate(info.buffer_size());
    (buf, info.width as usize)
}

/// Area-average downscale (premultiplied alpha, so edges don't darken).
fn downscale(src: &[u8], n: usize, m: usize) -> Vec<u8> {
    let scale = n as f32 / m as f32;
    let mut out = vec![0u8; m * m * 4];
    for y in 0..m {
        for x in 0..m {
            let (x0, x1) = ((x as f32 * scale) as usize, (((x + 1) as f32 * scale).ceil() as usize).min(n));
            let (y0, y1) = ((y as f32 * scale) as usize, (((y + 1) as f32 * scale).ceil() as usize).min(n));
            let mut acc = [0f32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = &src[(sy * n + sx) * 4..][..4];
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a;
                    acc[1] += p[1] as f32 * a;
                    acc[2] += p[2] as f32 * a;
                    acc[3] += a;
                }
            }
            let count = ((x1 - x0) * (y1 - y0)) as f32;
            let o = &mut out[(y * m + x) * 4..][..4];
            if acc[3] > 0.0 {
                for c in 0..3 {
                    o[c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            o[3] = (acc[3] / count * 255.0).round() as u8;
        }
    }
    out
}

fn main() {
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).to_path_buf();

    let (icon256, _) = decode("assets/haze-icon-pack/png/haze-256.png");
    std::fs::write(out.join("icon-256.rgba"), &icon256).expect("write icon");

    // macOS: artwork scaled into Apple's grid, centred on a transparent canvas.
    let (master, n) = decode("assets/haze-icon-pack/png/haze-1024.png");
    let canvas = 256usize;
    let art = (canvas as f32 * MACOS_ART_FRACTION).round() as usize;
    let small = downscale(&master, n, art);
    let offset = (canvas - art) / 2;
    let mut padded = vec![0u8; canvas * canvas * 4];
    for y in 0..art {
        let dst = ((y + offset) * canvas + offset) * 4;
        padded[dst..dst + art * 4].copy_from_slice(&small[y * art * 4..(y + 1) * art * 4]);
    }
    std::fs::write(out.join("icon-256-macos.rgba"), &padded).expect("write icon");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/haze-icon-pack/haze.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/haze-icon-pack/haze.ico");
        res.compile().expect("embed Windows icon");
    }
}
