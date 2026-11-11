//! The 2D renderer: everything on screen is a quad drawn by one shader — a rounded rectangle (its corners cut by a
//! signed distance in the fragment shader, so they stay smooth at any size), optionally textured with a picture or
//! with glyphs from the text atlas. Quads are batched until the texture changes, so a screen is a few draw calls.
//!
//! Coordinates are the UI's 1920x1080 logical pixels whatever the framebuffer is; text is rasterised at the
//! framebuffer's scale so it stays sharp.

pub mod text;

use anyhow::{Result, anyhow};
use glow::HasContext;

pub use text::{Font, TextAtlas};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl Color {
    pub const fn rgb(hex: u32) -> Color {
        Color(
            ((hex >> 16) & 0xff) as f32 / 255.0,
            ((hex >> 8) & 0xff) as f32 / 255.0,
            (hex & 0xff) as f32 / 255.0,
            1.0,
        )
    }

    pub fn alpha(self, alpha: f32) -> Color {
        Color(self.0, self.1, self.2, self.3 * alpha)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn inset(self, by: f32) -> Rect {
        Rect::new(self.x + by, self.y + by, self.w - 2.0 * by, self.h - 2.0 * by)
    }

    /// Grown around its centre by a factor (focus lift).
    pub fn scaled(self, factor: f32) -> Rect {
        let w = self.w * factor;
        let h = self.h * factor;
        Rect::new(self.x - (w - self.w) / 2.0, self.y - (h - self.h) / 2.0, w, h)
    }
}

/// A texture on the GPU and its size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texture {
    pub id: glow::NativeTexture,
    pub width: u32,
    pub height: u32,
}

/// Which GL the platform gave us: OpenGL ES 2 on the TV, desktop OpenGL (core profile) on the simulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlFlavor {
    Es2,
    DesktopCore,
}

/// Floats per vertex: position 2, uv 2, colour 4, position inside the quad 2, quad size 2, corner radius 1, mode 1.
const STRIDE: usize = 14;
const MODE_SOLID: f32 = 0.0;
const MODE_IMAGE: f32 = 1.0;
const MODE_TEXT: f32 = 2.0;

const VERTEX: &str = r#"
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
attribute vec2 a_local;
attribute vec2 a_size;
attribute vec2 a_shape;
uniform vec2 u_screen;
varying vec2 v_uv;
varying vec4 v_color;
varying vec2 v_local;
varying vec2 v_size;
varying vec2 v_shape;
void main() {
    v_uv = a_uv;
    v_color = a_color;
    v_local = a_local;
    v_size = a_size;
    v_shape = a_shape;
    vec2 clip = a_pos / u_screen * 2.0 - 1.0;
    gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
uniform sampler2D u_tex;
varying vec2 v_uv;
varying vec4 v_color;
varying vec2 v_local;
varying vec2 v_size;
varying vec2 v_shape;
void main() {
    vec4 c = v_color;
    if (v_shape.y > 1.5) {
        c.a *= texture2D(u_tex, v_uv).a;
    } else if (v_shape.y > 0.5) {
        c *= texture2D(u_tex, v_uv);
    }
    float r = v_shape.x;
    if (r > 0.0) {
        vec2 q = abs(v_local - v_size * 0.5) - v_size * 0.5 + r;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
        c.a *= clamp(0.5 - d, 0.0, 1.0);
    }
    gl_FragColor = c;
}
"#;

pub struct Renderer {
    pub gl: glow::Context,
    program: glow::NativeProgram,
    vbo: glow::NativeBuffer,
    u_screen: glow::NativeUniformLocation,
    white: Texture,
    current: Option<glow::NativeTexture>,
    vertices: Vec<f32>,
    pub text: TextAtlas,
    /// framebuffer pixels per logical pixel
    pub scale: f32,
    fb: (u32, u32),
    pub draw_calls: u32,
}

pub const SCREEN_W: f32 = 1920.0;
pub const SCREEN_H: f32 = 1080.0;

impl Renderer {
    /// # Safety
    /// The GL context must be current on this thread for the renderer's whole life.
    pub unsafe fn new(gl: glow::Context, flavor: GlFlavor) -> Result<Renderer> {
        unsafe {
            let (vs_head, fs_head) = match flavor {
                GlFlavor::Es2 => ("#version 100\n", "#version 100\nprecision mediump float;\n"),
                GlFlavor::DesktopCore => (
                    "#version 150\n#define attribute in\n#define varying out\n",
                    "#version 150\n#define varying in\n#define texture2D texture\nout vec4 frag_color;\n#define gl_FragColor frag_color\n",
                ),
            };
            let program = gl.create_program().map_err(|e| anyhow!(e))?;
            for (kind, head, source) in [
                (glow::VERTEX_SHADER, vs_head, VERTEX),
                (glow::FRAGMENT_SHADER, fs_head, FRAGMENT),
            ] {
                let shader = gl.create_shader(kind).map_err(|e| anyhow!(e))?;
                gl.shader_source(shader, &format!("{head}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(anyhow!("shader: {}", gl.get_shader_info_log(shader)));
                }
                gl.attach_shader(program, shader);
            }
            for (index, name) in ["a_pos", "a_uv", "a_color", "a_local", "a_size", "a_shape"].iter().enumerate() {
                gl.bind_attrib_location(program, index as u32, name);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(anyhow!("program: {}", gl.get_program_info_log(program)));
            }
            gl.use_program(Some(program));
            let u_screen = gl.get_uniform_location(program, "u_screen").ok_or_else(|| anyhow!("u_screen"))?;
            if let Some(u_tex) = gl.get_uniform_location(program, "u_tex") {
                gl.uniform_1_i32(Some(&u_tex), 0);
            }
            if flavor == GlFlavor::DesktopCore {
                let vao = gl.create_vertex_array().map_err(|e| anyhow!(e))?;
                gl.bind_vertex_array(Some(vao));
            }
            let vbo = gl.create_buffer().map_err(|e| anyhow!(e))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (STRIDE * 4) as i32;
            let mut offset = 0;
            for (index, size) in [2, 2, 4, 2, 2, 2].iter().enumerate() {
                gl.enable_vertex_attrib_array(index as u32);
                gl.vertex_attrib_pointer_f32(index as u32, *size, glow::FLOAT, false, stride, offset);
                offset += size * 4;
            }
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let white = upload(&gl, 1, 1, &[255, 255, 255, 255])?;
            let text = TextAtlas::new(&gl)?;
            Ok(Renderer {
                gl,
                program,
                vbo,
                u_screen,
                white,
                current: None,
                vertices: Vec::with_capacity(STRIDE * 6 * 512),
                text,
                scale: 1.0,
                fb: (1920, 1080),
                draw_calls: 0,
            })
        }
    }

    /// The platform tells the renderer the framebuffer's size (before each frame).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.fb = (width.max(1), height.max(1));
    }

    pub fn framebuffer(&self) -> (u32, u32) {
        self.fb
    }

    /// Starts a frame, cleared to `clear`.
    pub fn begin(&mut self, clear: Color) {
        let (fb_width, fb_height) = self.fb;
        self.scale = fb_height as f32 / SCREEN_H;
        self.draw_calls = 0;
        unsafe {
            self.gl.viewport(0, 0, fb_width as i32, fb_height as i32);
            self.gl.clear_color(clear.0, clear.1, clear.2, clear.3);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.use_program(Some(self.program));
            self.gl.uniform_2_f32(Some(&self.u_screen), SCREEN_W, SCREEN_H);
        }
        self.text.set_scale(self.scale);
    }

    pub fn end(&mut self) {
        self.flush();
        self.text.upload_pending(&self.gl);
    }

    fn use_texture(&mut self, texture: glow::NativeTexture) {
        if self.current != Some(texture) {
            self.flush();
            self.current = Some(texture);
        }
    }

    fn flush(&mut self) {
        if self.vertices.is_empty() {
            return;
        }
        // glyphs added this frame must be on the GPU before text that uses them is drawn
        self.text.upload_pending(&self.gl);
        unsafe {
            let gl = &self.gl;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.current.or(Some(self.white.id)));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let bytes = std::slice::from_raw_parts(self.vertices.as_ptr() as *const u8, self.vertices.len() * 4);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.draw_arrays(glow::TRIANGLES, 0, (self.vertices.len() / STRIDE) as i32);
        }
        self.draw_calls += 1;
        self.vertices.clear();
    }

    fn quad(&mut self, r: Rect, uv: [f32; 4], color: Color, radius: f32, mode: f32) {
        self.quad_colors(r, uv, [color; 4], radius, mode);
    }

    /// A quad with a colour per corner (top-left, top-right, bottom-left, bottom-right): gradients.
    fn quad_colors(&mut self, r: Rect, uv: [f32; 4], colors: [Color; 4], radius: f32, mode: f32) {
        // past half the short side the distance field has no inside: a circle is the most it can be
        let radius = radius.min(r.w.min(r.h) / 2.0);
        let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
        let corners = [
            (r.x, r.y, u0, v0, 0.0, 0.0),
            (r.x + r.w, r.y, u1, v0, r.w, 0.0),
            (r.x, r.y + r.h, u0, v1, 0.0, r.h),
            (r.x + r.w, r.y + r.h, u1, v1, r.w, r.h),
        ];
        for &index in &[0usize, 1, 2, 2, 1, 3] {
            let (x, y, u, v, lx, ly) = corners[index];
            let c = colors[index];
            self.vertices.extend_from_slice(&[x, y, u, v, c.0, c.1, c.2, c.3, lx, ly, r.w, r.h, radius, mode]);
        }
    }

    /// A linear gradient from `from` to `to`, left to right or top to bottom.
    pub fn gradient(&mut self, r: Rect, from: Color, to: Color, vertical: bool) {
        let white = self.white.id;
        self.use_texture(white);
        let colors = if vertical { [from, from, to, to] } else { [from, to, from, to] };
        self.quad_colors(r, [0.0, 0.0, 1.0, 1.0], colors, 0.0, MODE_SOLID);
    }

    /// A filled rectangle with rounded corners.
    pub fn rect(&mut self, r: Rect, radius: f32, color: Color) {
        let white = self.white.id;
        self.use_texture(white);
        self.quad(r, [0.0, 0.0, 1.0, 1.0], color, radius, MODE_SOLID);
    }

    /// A picture filling `r` (cropped to its aspect ratio, like CSS `object-fit: cover`).
    pub fn image(&mut self, texture: &Texture, r: Rect, radius: f32, alpha: f32) {
        let image_aspect = texture.width as f32 / texture.height.max(1) as f32;
        let rect_aspect = r.w / r.h.max(1.0);
        let uv = if image_aspect > rect_aspect {
            let share = rect_aspect / image_aspect;
            [(1.0 - share) / 2.0, 0.0, (1.0 + share) / 2.0, 1.0]
        } else {
            let share = image_aspect / rect_aspect;
            [0.0, (1.0 - share) / 2.0, 1.0, (1.0 + share) / 2.0]
        };
        self.use_texture(texture.id);
        self.quad(r, uv, Color(1.0, 1.0, 1.0, alpha), radius, MODE_IMAGE);
    }

    /// A picture's part `uv` stretched over `r` (backdrops placed by hand).
    pub fn image_uv(&mut self, texture: &Texture, r: Rect, uv: [f32; 4], alpha: f32) {
        self.use_texture(texture.id);
        self.quad(r, uv, Color(1.0, 1.0, 1.0, alpha), 0.0, MODE_IMAGE);
    }

    /// One line of text; `x, y` is the top-left of its line box. Cut with an ellipsis past `max_width`.
    /// Returns the width drawn.
    pub fn text(&mut self, font: Font, size: f32, x: f32, y: f32, text: &str, color: Color, max_width: Option<f32>) -> f32 {
        let glyphs = self.text.layout_line(font, size, text, max_width);
        let atlas = self.text.texture();
        self.use_texture(atlas.id);
        let ascent = self.text.ascent(font, size);
        let mut width = 0.0;
        for glyph in &glyphs {
            if let Some(cell) = glyph.cell {
                let r = Rect::new(x + glyph.x + cell.left, y + ascent - cell.top, cell.width, cell.height);
                self.quad(r, cell.uv, color, 0.0, MODE_TEXT);
            }
            width = glyph.x + glyph.advance;
        }
        width
    }

    /// Text wrapped to `max_width`, at most `max_lines` lines (the last one cut with an ellipsis). Returns the
    /// height used.
    #[allow(clippy::too_many_arguments)]
    pub fn paragraph(
        &mut self,
        font: Font,
        size: f32,
        x: f32,
        y: f32,
        text: &str,
        color: Color,
        max_width: f32,
        max_lines: usize,
        line_height: f32,
    ) -> f32 {
        let lines = self.text.wrap(font, size, text, max_width, max_lines);
        for (index, line) in lines.iter().enumerate() {
            self.text(font, size, x, y + index as f32 * line_height, line, color, Some(max_width));
        }
        lines.len() as f32 * line_height
    }

    pub fn measure(&mut self, font: Font, size: f32, text: &str) -> f32 {
        self.text.measure(font, size, text)
    }

    /// Uploads a decoded picture (RGBA, top row first).
    pub fn upload(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Texture> {
        self.flush();
        unsafe { upload(&self.gl, width, height, rgba) }
    }

    pub fn delete(&mut self, texture: Texture) {
        self.flush();
        if self.current == Some(texture.id) {
            self.current = None;
        }
        unsafe { self.gl.delete_texture(texture.id) };
    }
}

/// # Safety
/// The GL context must be current.
unsafe fn upload(gl: &glow::Context, width: u32, height: u32, rgba: &[u8]) -> Result<Texture> {
    unsafe {
        let id = gl.create_texture().map_err(|e| anyhow!(e))?;
        gl.bind_texture(glow::TEXTURE_2D, Some(id));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
        Ok(Texture { id, width, height })
    }
}
