//! Drawing the picture: the frame's Y/U/V planes go to the GPU as they are and a
//! shader turns them into colour, scaled to the window by the texture sampler.

use crate::media::VideoFrame;
use crate::media::frame::Layout;
use eframe::egui::{self, Rect};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use std::sync::{Arc, Mutex};

/// GPU side of the picture, shared with the paint callback.
pub struct Video {
    inner: Arc<Mutex<Inner>>,
    /// Serial of the frame handed over last.
    serial: u64,
}

#[derive(Default)]
struct Inner {
    gl: Option<Gl>,
    /// A frame waiting to be uploaded.
    pending: Option<Arc<VideoFrame>>,
    /// What's on the GPU: width, height, layout; none before the first frame.
    uploaded: Option<(u32, u32, Layout)>,
    matrix: [f32; 9],
    offset: [f32; 3],
    failed: bool,
}

struct Gl {
    program: glow::Program,
    vao: glow::VertexArray,
    textures: [glow::Texture; 3],
}

impl Video {
    pub fn new() -> Video {
        Video { inner: Arc::default(), serial: 0 }
    }

    /// Hand over the latest frame, if it's new.
    pub fn set_frame(&mut self, serial: u64, frame: Option<Arc<VideoFrame>>) {
        if serial == self.serial {
            return;
        }
        self.serial = serial;
        if let Some(f) = frame {
            self.inner.lock().unwrap_or_else(|e| e.into_inner()).pending = Some(f);
        }
    }

    /// Forget the picture (another channel starts).
    pub fn clear(&mut self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.pending = None;
        inner.uploaded = None;
        self.serial = 0;
    }

    pub fn has_picture(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.uploaded.is_some() || inner.pending.is_some()
    }

    /// Paint the picture into `rect` (already fitted to its aspect ratio).
    pub fn paint(&self, painter: &egui::Painter, rect: Rect) {
        let inner = self.inner.clone();
        let callback = egui_glow::CallbackFn::new(move |_info, painter| {
            let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: called on the GL thread with egui_glow's current context.
            unsafe { inner.draw(painter.gl()) };
        });
        painter.add(egui::PaintCallback { rect, callback: Arc::new(callback) });
    }
}

impl Inner {
    unsafe fn draw(&mut self, gl: &glow::Context) {
        if self.failed {
            return;
        }
        if self.gl.is_none() {
            match unsafe { Gl::new(gl) } {
                Ok(g) => self.gl = Some(g),
                Err(e) => {
                    log_error!("video shader: {e}");
                    self.failed = true;
                    return;
                }
            }
        }
        if let Some(frame) = self.pending.take() {
            unsafe { self.upload(gl, &frame) };
        }
        let (Some(g), Some((_, _, layout))) = (&self.gl, self.uploaded) else { return };
        unsafe {
            gl.use_program(Some(g.program));
            gl.bind_vertex_array(Some(g.vao));
            gl.disable(glow::BLEND);
            for (i, t) in g.textures.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(*t));
            }
            let loc = |n: &str| gl.get_uniform_location(g.program, n);
            gl.uniform_1_i32(loc("y_tex").as_ref(), 0);
            gl.uniform_1_i32(loc("u_tex").as_ref(), 1);
            gl.uniform_1_i32(loc("v_tex").as_ref(), 2);
            gl.uniform_1_i32(loc("nv12").as_ref(), i32::from(layout == Layout::Nv12));
            gl.uniform_matrix_3_f32_slice(loc("yuv_to_rgb").as_ref(), false, &self.matrix);
            gl.uniform_3_f32_slice(loc("yuv_offset").as_ref(), &self.offset);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, f: &VideoFrame) {
        let Some(g) = &self.gl else { return };
        let (w, h) = (f.width as i32, f.height as i32);
        let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
        let planes: Vec<(i32, i32, u32, u32)> = match f.layout {
            Layout::I420 => vec![(w, h, glow::R8, glow::RED), (cw, ch, glow::R8, glow::RED), (cw, ch, glow::R8, glow::RED)],
            Layout::Nv12 => vec![(w, h, glow::R8, glow::RED), (cw, ch, glow::RG8, glow::RG)],
        };
        let same = self.uploaded == Some((f.width, f.height, f.layout));
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for (i, (pw, ph, internal, format)) in planes.into_iter().enumerate() {
                gl.bind_texture(glow::TEXTURE_2D, Some(g.textures[i]));
                let data = glow::PixelUnpackData::Slice(Some(&f.planes[i]));
                if same {
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, pw, ph, format, glow::UNSIGNED_BYTE, data);
                } else {
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, pw, ph, 0, format, glow::UNSIGNED_BYTE, data);
                }
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
        self.uploaded = Some((f.width, f.height, f.layout));
        (self.matrix, self.offset) = colour_conversion(f.bt709, f.full_range);
    }
}

/// The YUV → RGB matrix (column-major) and the offsets taken off first.
fn colour_conversion(bt709: bool, full_range: bool) -> ([f32; 9], [f32; 3]) {
    let (kr, kb) = if bt709 { (0.2126, 0.0722) } else { (0.299, 0.114) };
    let kg = 1.0 - kr - kb;
    let (ys, cs, yo) = if full_range { (1.0, 1.0, 0.0) } else { (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0) };
    let rv = 2.0 * (1.0 - kr) * cs;
    let bu = 2.0 * (1.0 - kb) * cs;
    let gu = -bu * kb / kg;
    let gv = -rv * kr / kg;
    // Columns: Y, U, V contributions to (R, G, B).
    ([ys, ys, ys, 0.0, gu, bu, rv, gv, 0.0], [yo, 128.0 / 255.0, 128.0 / 255.0])
}

impl Gl {
    unsafe fn new(gl: &glow::Context) -> Result<Gl, String> {
        let version = egui_glow::ShaderVersion::get(gl);
        if !version.is_new_shader_interface() {
            return Err(format!("{version:?} is too old"));
        }
        let header = format!("{}{}", version.version_declaration(), if version.is_embedded() { "precision mediump float;\n" } else { "" });
        let vertex = format!(
            "{header}out vec2 uv;
            void main() {{
                vec2 p = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
                uv = vec2(p.x, 1.0 - p.y);
                gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
            }}"
        );
        let fragment = format!(
            "{header}in vec2 uv;
            out vec4 colour;
            uniform sampler2D y_tex;
            uniform sampler2D u_tex;
            uniform sampler2D v_tex;
            uniform int nv12;
            uniform mat3 yuv_to_rgb;
            uniform vec3 yuv_offset;
            void main() {{
                float y = texture(y_tex, uv).r;
                vec2 c = nv12 == 1 ? texture(u_tex, uv).rg : vec2(texture(u_tex, uv).r, texture(v_tex, uv).r);
                vec3 rgb = yuv_to_rgb * (vec3(y, c) - yuv_offset);
                colour = vec4(clamp(rgb, 0.0, 1.0), 1.0);
            }}"
        );
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, &vertex), (glow::FRAGMENT_SHADER, &fragment)] {
                let s = gl.create_shader(kind)?;
                gl.shader_source(s, src);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    return Err(gl.get_shader_info_log(s));
                }
                gl.attach_shader(program, s);
                shaders.push(s);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            let vao = gl.create_vertex_array()?;
            let make_texture = || -> Result<glow::Texture, String> {
                let t = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                for (k, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, k, v as i32);
                }
                Ok(t)
            };
            Ok(Gl { program, vao, textures: [make_texture()?, make_texture()?, make_texture()?] })
        }
    }
}

/// The largest rect of aspect `aspect` centred in `area`.
pub fn fit(area: Rect, aspect: f32) -> Rect {
    if aspect <= 0.0 {
        return area;
    }
    let (w, h) = (area.width(), area.height());
    let size = if w / h > aspect { egui::vec2(h * aspect, h) } else { egui::vec2(w, w / aspect) };
    Rect::from_center_size(area.center(), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &[f32; 9], o: &[f32; 3], yuv: [f32; 3]) -> [f32; 3] {
        let v = [yuv[0] - o[0], yuv[1] - o[1], yuv[2] - o[2]];
        [0, 1, 2].map(|r| m[r] * v[0] + m[3 + r] * v[1] + m[6 + r] * v[2])
    }

    #[test]
    fn converts_tv_range_white_black_and_red() {
        let (m, o) = colour_conversion(false, false);
        let white = apply(&m, &o, [235.0 / 255.0, 0.5019608, 0.5019608]);
        let black = apply(&m, &o, [16.0 / 255.0, 0.5019608, 0.5019608]);
        assert!(white.iter().all(|c| (c - 1.0).abs() < 0.01), "{white:?}");
        assert!(black.iter().all(|c| c.abs() < 0.01), "{black:?}");
        // BT.601 pure red: Y 81, Cb 90, Cr 240.
        let red = apply(&m, &o, [81.0 / 255.0, 90.0 / 255.0, 240.0 / 255.0]);
        assert!((red[0] - 1.0).abs() < 0.02 && red[1].abs() < 0.02 && red[2].abs() < 0.02, "{red:?}");
    }

    #[test]
    fn fits_by_aspect() {
        let r = fit(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0)), 1.0);
        assert_eq!(r, Rect::from_min_size(egui::pos2(50.0, 0.0), egui::vec2(100.0, 100.0)));
    }
}
