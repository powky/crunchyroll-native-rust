//! The 2D renderer: everything on screen is a quad drawn by one shader — a rounded rectangle (its corners cut by a
//! signed distance in the fragment shader, so they stay smooth at any size), optionally textured with a picture or
//! with glyphs from the text atlas. Quads are batched until the texture changes, so a screen is a few draw calls.
//! A frame is recorded first and drawn at its end, and only when it differs from the last one drawn: a screen at
//! rest costs the GPU nothing, and the CPU only the recording.
//!
//! Coordinates are the UI's 1920x1080 logical pixels whatever the framebuffer is; text is rasterised at the
//! framebuffer's scale so it stays sharp.

pub mod icons;
pub mod text;

use anyhow::{Result, anyhow};
use glow::HasContext;

pub use icons::Icon;
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

/// How a picture's pixels are laid out: 4 bytes each, or 2 for big opaque ones (half the memory, and half the
/// reading a frame: at 4K a backdrop is the most of what the GPU reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba,
    Rgb565,
}

impl PixelFormat {
    pub fn bytes(self) -> usize {
        match self {
            PixelFormat::Rgba => 4,
            PixelFormat::Rgb565 => 2,
        }
    }

    fn gl(self) -> (u32, u32) {
        match self {
            PixelFormat::Rgba => (glow::RGBA, glow::UNSIGNED_BYTE),
            PixelFormat::Rgb565 => (glow::RGB, glow::UNSIGNED_SHORT_5_6_5),
        }
    }
}

/// A picture to make into a texture off the drawing thread, and the key it comes back with.
pub struct UploadJob {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub pixels: Vec<u8>,
}

/// Textures made on a thread with its own GL context, sharing objects with the renderer's. This TV's driver spends
/// ~20 ms in each upload whatever its size (it rearranges the pixels on the CPU): on the drawing thread that's a
/// lost frame.
pub struct Uploads {
    jobs: std::sync::mpsc::Sender<UploadJob>,
    done: std::sync::mpsc::Receiver<(String, Result<Texture>, usize)>,
}

impl Uploads {
    /// Starts the thread. `make_current` runs on it first, makes its context current and returns its GL; this
    /// returns once it has.
    pub fn start(make_current: impl FnOnce() -> Result<glow::Context> + Send + 'static) -> Result<Uploads> {
        let (jobs, incoming) = std::sync::mpsc::channel::<UploadJob>();
        let (finished, done) = std::sync::mpsc::channel();
        let (ready_tx, ready) = std::sync::mpsc::channel::<Result<()>>();
        std::thread::Builder::new().name("uploads".into()).spawn(move || {
            let gl = match make_current() {
                Ok(gl) => gl,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1) };
            let _ = ready_tx.send(Ok(()));
            for job in incoming {
                let texture = unsafe { upload(&gl, job.width, job.height, job.format, Some(&job.pixels)) };
                // finished on the GPU before the drawing thread can draw with it
                unsafe { gl.finish() };
                if finished.send((job.key, texture, job.pixels.len())).is_err() {
                    break;
                }
            }
        })?;
        ready.recv().map_err(|_| anyhow!("the upload thread ended"))??;
        Ok(Uploads { jobs, done })
    }

    pub fn send(&self, job: UploadJob) {
        let _ = self.jobs.send(job);
    }

    /// The textures made since the last call: key, texture, bytes.
    pub fn finished(&self) -> Vec<(String, Result<Texture>, usize)> {
        self.done.try_iter().collect()
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
/// An outline: the mode is 3 plus the line's width in UI pixels.
const MODE_RING: f32 = 3.0;

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
    float mode = v_shape.y;
    if (mode > 2.5) {
    } else if (mode > 1.5) {
        c.a *= texture2D(u_tex, v_uv).a;
    } else if (mode > 0.5) {
        c *= texture2D(u_tex, v_uv);
    }
    float r = v_shape.x;
    if (r > 0.0 || mode > 2.5) {
        vec2 q = abs(v_local - v_size * 0.5) - v_size * 0.5 + r;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
        c.a *= clamp(0.5 - d, 0.0, 1.0);
        if (mode > 2.5) {
            c.a *= clamp(0.5 + d + (mode - 3.0), 0.0, 1.0);
        }
    }
    gl_FragColor = c;
}
"#;

/// A stretch of the frame's quads drawn with one texture, or where drawing is clipped from there on (a GL scissor).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Draw { texture: glow::NativeTexture, first: i32, count: i32 },
    Clip(Option<[i32; 4]>),
}

pub struct Renderer {
    pub gl: glow::Context,
    program: glow::NativeProgram,
    vbo: glow::NativeBuffer,
    u_screen: glow::NativeUniformLocation,
    white: Texture,
    current: Option<glow::NativeTexture>,
    /// the frame being recorded: its quads, what draws them, and the colour it starts from
    vertices: Vec<f32>,
    steps: Vec<Step>,
    /// where the quads of the batch being recorded start
    batch_start: usize,
    clear: Color,
    /// the last frame drawn, to tell whether this one is any different
    last_vertices: Vec<f32>,
    last_steps: Vec<Step>,
    last_clear: Color,
    last_fb: (u32, u32),
    /// a texture changed since the last frame drawn: the next one draws whatever it looks like
    changed: bool,
    /// the frame just ended was drawn (else the screen still shows the last one, and there's nothing to present)
    pub presented: bool,
    /// draw every frame, the same or not (the simulator, whose screenshots read the frame back)
    pub always_draw: bool,
    pub text: TextAtlas,
    /// framebuffer pixels per logical pixel
    pub scale: f32,
    fb: (u32, u32),
    pub draw_calls: u32,
    /// area drawn this frame, in UI pixels (1920x1080 is one screen)
    pub fill: f32,
    /// time the frame's first GL call waited (for a buffer the compositor gave back)
    pub waited: std::time::Duration,
    /// a thread uploading pictures, where the platform can have one (else they go up in strips between frames)
    pub uploads: Option<Uploads>,
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
            // coverage adds up as "over" in the alpha channel too: the compositor lays the frame over the video by it,
            // and squaring it (plain SRC_ALPHA blending) let a fade meant to be 80% dark show 64%
            gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let white = upload(&gl, 1, 1, PixelFormat::Rgba, Some(&[255, 255, 255, 255]))?;
            let text = TextAtlas::new(&gl)?;
            Ok(Renderer {
                gl,
                program,
                vbo,
                u_screen,
                white,
                current: None,
                vertices: Vec::with_capacity(STRIDE * 6 * 512),
                steps: Vec::new(),
                batch_start: 0,
                clear: Color(0.0, 0.0, 0.0, 0.0),
                last_vertices: Vec::new(),
                last_steps: Vec::new(),
                last_clear: Color(0.0, 0.0, 0.0, 0.0),
                last_fb: (0, 0),
                changed: true,
                presented: false,
                always_draw: false,
                text,
                scale: 1.0,
                fb: (1920, 1080),
                draw_calls: 0,
                fill: 0.0,
                waited: std::time::Duration::ZERO,
                uploads: None,
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

    /// Starts recording a frame, cleared to `clear`.
    pub fn begin(&mut self, clear: Color) {
        let (_, fb_height) = self.fb;
        self.scale = fb_height as f32 / SCREEN_H;
        self.draw_calls = 0;
        self.fill = 0.0;
        self.clear = clear;
        self.vertices.clear();
        self.steps.clear();
        self.batch_start = 0;
        self.current = None;
        self.text.set_scale(&self.gl, self.scale);
    }

    /// Ends the frame and draws it, unless it's the same as the last one drawn (that's still on screen). Returns
    /// whether it drew, so whether there's a frame to present.
    pub fn end(&mut self) -> bool {
        self.clip(None);
        let same = !self.always_draw
            && !self.changed
            && !self.text.has_pending()
            && self.clear == self.last_clear
            && self.fb == self.last_fb
            && self.steps == self.last_steps
            && bytes(&self.vertices) == bytes(&self.last_vertices);
        self.presented = !same;
        if same {
            self.waited = std::time::Duration::ZERO;
            return false;
        }
        self.changed = false;
        // glyphs added this frame go to the GPU before the text that uses them is drawn
        self.text.upload_pending(&self.gl);
        let (fb_width, fb_height) = self.fb;
        let gl = &self.gl;
        unsafe {
            let started = std::time::Instant::now();
            gl.viewport(0, 0, fb_width as i32, fb_height as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(self.clear.0, self.clear.1, self.clear.2, self.clear.3);
            gl.clear(glow::COLOR_BUFFER_BIT);
            self.waited = started.elapsed();
            gl.use_program(Some(self.program));
            gl.uniform_2_f32(Some(&self.u_screen), SCREEN_W, SCREEN_H);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&self.vertices), glow::STREAM_DRAW);
            for step in &self.steps {
                match *step {
                    Step::Draw { texture, first, count } => {
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        gl.draw_arrays(glow::TRIANGLES, first, count);
                    }
                    Step::Clip(Some([x, y, w, h])) => {
                        gl.enable(glow::SCISSOR_TEST);
                        gl.scissor(x, y, w, h);
                    }
                    Step::Clip(None) => gl.disable(glow::SCISSOR_TEST),
                }
            }
        }
        std::mem::swap(&mut self.vertices, &mut self.last_vertices);
        std::mem::swap(&mut self.steps, &mut self.last_steps);
        self.last_clear = self.clear;
        self.last_fb = self.fb;
        true
    }

    /// Something drawn from changed without the frame's quads showing it (a picture uploaded off the drawing
    /// thread): the next frame draws.
    pub fn invalidate(&mut self) {
        self.changed = true;
    }

    /// Draws only inside `r` (in UI coordinates) until the next call; `None` draws everywhere again.
    pub fn clip(&mut self, r: Option<Rect>) {
        self.flush();
        let scissor = r.map(|r| {
            let s = self.scale;
            let (_, fb_height) = self.fb;
            let x = (r.x * s).floor().max(0.0) as i32;
            let w = (r.w * s).ceil().max(0.0) as i32;
            let h = (r.h * s).ceil().max(0.0) as i32;
            // GL counts rows from the bottom
            let y = fb_height as i32 - ((r.y + r.h) * s).ceil() as i32;
            [x, y.max(0), w, h]
        });
        self.steps.push(Step::Clip(scissor));
    }

    fn use_texture(&mut self, texture: glow::NativeTexture) {
        if self.current != Some(texture) {
            self.flush();
            self.current = Some(texture);
        }
    }

    /// Closes the batch being recorded (its quads all use the current texture).
    fn flush(&mut self) {
        let end = self.vertices.len();
        if end == self.batch_start {
            return;
        }
        self.steps.push(Step::Draw {
            texture: self.current.unwrap_or(self.white.id),
            first: (self.batch_start / STRIDE) as i32,
            count: ((end - self.batch_start) / STRIDE) as i32,
        });
        self.batch_start = end;
        self.draw_calls += 1;
    }

    fn quad(&mut self, r: Rect, uv: [f32; 4], color: Color, radius: f32, mode: f32) {
        self.quad_colors(r, uv, [color; 4], radius, mode);
    }

    /// A quad with a colour per corner (top-left, top-right, bottom-left, bottom-right): gradients.
    fn quad_colors(&mut self, r: Rect, uv: [f32; 4], colors: [Color; 4], radius: f32, mode: f32) {
        // past half the short side the distance field has no inside: a circle is the most it can be
        let radius = radius.min(r.w.min(r.h) / 2.0);
        self.fill += r.w * r.h;
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

    /// A gradient in a rounded rectangle.
    pub fn gradient_rounded(&mut self, r: Rect, radius: f32, from: Color, to: Color, vertical: bool) {
        self.use_texture(self.white.id);
        let colors = if vertical { [from, from, to, to] } else { [from, to, from, to] };
        self.quad_colors(r, [0.0, 0.0, 1.0, 1.0], colors, radius, MODE_SOLID);
    }

    /// A UI coordinate moved to the nearest screen pixel.
    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    /// A filled rectangle with rounded corners.
    pub fn rect(&mut self, r: Rect, radius: f32, color: Color) {
        let white = self.white.id;
        self.use_texture(white);
        self.quad(r, [0.0, 0.0, 1.0, 1.0], color, radius, MODE_SOLID);
    }

    /// The outline of a rounded rectangle: a line `width` UI pixels wide just inside `r`'s edge.
    pub fn ring(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        let white = self.white.id;
        self.use_texture(white);
        self.quad(r, [0.0, 0.0, 1.0, 1.0], color, radius, MODE_RING + width);
    }

    /// A picture filling `r` (cropped to its aspect ratio, like CSS `object-fit: cover`).
    pub fn image(&mut self, texture: &Texture, r: Rect, radius: f32, alpha: f32) {
        let uv = cover(texture, r);
        self.use_texture(texture.id);
        self.quad(r, uv, Color(1.0, 1.0, 1.0, alpha), radius, MODE_IMAGE);
    }

    /// A picture filling `r` like [`Renderer::image`], let through by `keep(x, y)` (0..=1, at fractions of `r`) so
    /// what's under it shows instead: a backdrop and its shading in one layer, where a shade drawn over it would
    /// cost a layer of its own (at 4K each layer is 8 million pixels). `xs` and `ys` are fractions where `keep`
    /// bends; between them it's drawn as a grid fine enough to follow it, and cells it hides entirely are skipped.
    pub fn image_faded(&mut self, texture: &Texture, r: Rect, alpha: f32, xs: &[f32], ys: &[f32], keep: impl Fn(f32, f32) -> f32) {
        const SPLIT: usize = 4;
        let fine = |stops: &[f32]| -> Vec<f32> {
            let mut out = vec![0.0];
            for pair in stops.windows(2) {
                for step in 1..=SPLIT {
                    out.push(pair[0] + (pair[1] - pair[0]) * step as f32 / SPLIT as f32);
                }
            }
            out
        };
        let (xs, ys) = (fine(xs), fine(ys));
        let [u0, v0, u1, v1] = cover(texture, r);
        self.use_texture(texture.id);
        for row in ys.windows(2) {
            for column in xs.windows(2) {
                let corners = [(column[0], row[0]), (column[1], row[0]), (column[0], row[1]), (column[1], row[1])];
                let keeps = corners.map(|(x, y)| keep(x, y).clamp(0.0, 1.0) * alpha);
                if keeps.iter().all(|k| *k <= 0.002) {
                    continue;
                }
                let cell = Rect::new(r.x + r.w * column[0], r.y + r.h * row[0], r.w * (column[1] - column[0]), r.h * (row[1] - row[0]));
                let uv = [u0 + (u1 - u0) * column[0], v0 + (v1 - v0) * row[0], u0 + (u1 - u0) * column[1], v0 + (v1 - v0) * row[1]];
                self.quad_colors(cell, uv, keeps.map(|k| Color(1.0, 1.0, 1.0, k)), 0.0, MODE_IMAGE);
            }
        }
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
                // on whole screen pixels: a glyph between two pixel rows gets smeared across both
                let r = Rect::new(self.snap(x + glyph.x + cell.left), self.snap(y + ascent - cell.top), cell.width, cell.height);
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

    /// An icon in a `size`-tall box whose top-left is at `x`, `y` (the logo is wider than tall). Returns its width.
    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Color) -> f32 {
        let Some(cell) = self.text.icon(icon, size) else { return size * icon.aspect() };
        let atlas = self.text.texture();
        self.use_texture(atlas.id);
        let r = Rect::new(self.snap(x), self.snap(y), cell.width, cell.height);
        self.quad(r, cell.uv, color, 0.0, MODE_TEXT);
        cell.width
    }

    /// An icon centred on a point.
    pub fn icon_centered(&mut self, icon: Icon, cx: f32, cy: f32, size: f32, color: Color) {
        let w = size * icon.aspect();
        self.icon(icon, cx - w / 2.0, cy - size / 2.0, size, color);
    }

    /// The lines `text` wraps into at `max_width`, at most `max_lines`.
    pub fn wrap(&mut self, font: Font, size: f32, text: &str, max_width: f32, max_lines: usize) -> Vec<String> {
        self.text.wrap(font, size, text, max_width, max_lines)
    }

    pub fn measure(&mut self, font: Font, size: f32, text: &str) -> f32 {
        self.text.measure(font, size, text)
    }

    /// Uploads a decoded picture (RGBA, top row first).
    /// A texture with room for a picture, filled a few rows at a time with [`Renderer::upload_rows`].
    pub fn create_texture(&mut self, width: u32, height: u32, format: PixelFormat) -> Result<Texture> {
        self.changed = true;
        unsafe { upload(&self.gl, width, height, format, None) }
    }

    /// Rows `y..y + rows` of a picture (top row first) into a texture from [`Renderer::create_texture`].
    pub fn upload_rows(&mut self, texture: &Texture, y: u32, rows: u32, format: PixelFormat, pixels: &[u8]) {
        self.changed = true;
        let (layout, kind) = format.gl();
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.id));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                y as i32,
                texture.width as i32,
                rows as i32,
                layout,
                kind,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
        }
    }

    /// Deletes a texture (between frames: a frame being recorded may still draw with it).
    pub fn delete(&mut self, texture: Texture) {
        self.changed = true;
        unsafe { self.gl.delete_texture(texture.id) };
    }
}

/// A frame's vertices as bytes, for GL and for telling two frames apart.
fn bytes(vertices: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any bit pattern is a valid u8
    unsafe { std::slice::from_raw_parts(vertices.as_ptr() as *const u8, std::mem::size_of_val(vertices)) }
}

/// The part of a picture that covers `r` at the picture's own aspect (CSS `object-fit: cover`), as uv.
fn cover(texture: &Texture, r: Rect) -> [f32; 4] {
    let image_aspect = texture.width as f32 / texture.height.max(1) as f32;
    let rect_aspect = r.w / r.h.max(1.0);
    if image_aspect > rect_aspect {
        let share = rect_aspect / image_aspect;
        [(1.0 - share) / 2.0, 0.0, (1.0 + share) / 2.0, 1.0]
    } else {
        let share = image_aspect / rect_aspect;
        [0.0, (1.0 - share) / 2.0, 1.0, (1.0 + share) / 2.0]
    }
}

/// # Safety
/// The GL context must be current.
unsafe fn upload(gl: &glow::Context, width: u32, height: u32, format: PixelFormat, pixels: Option<&[u8]>) -> Result<Texture> {
    let (layout, kind) = format.gl();
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
            layout as i32,
            width as i32,
            height as i32,
            0,
            layout,
            kind,
            glow::PixelUnpackData::Slice(pixels),
        );
        Ok(Texture { id, width, height })
    }
}
