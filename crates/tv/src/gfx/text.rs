//! Text: Inter rasterised by fontdue into one atlas texture, glyph by glyph as they're first drawn, at the
//! framebuffer's pixel size so text is sharp. Layout is single lines (with an ellipsis) and greedy word wrap — all
//! the UI needs.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use fontdue::{Font as FontFace, FontSettings};
use glow::HasContext;

use super::Texture;

const ATLAS: u32 = 1024;
const PAD: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Regular,
    SemiBold,
    Bold,
}

/// Where a glyph is in the atlas, in logical pixels relative to the pen position and baseline.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub uv: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub x: f32,
    pub advance: f32,
    pub cell: Option<Cell>,
}

#[derive(Clone, Copy, Debug)]
struct Glyph {
    cell: Option<Cell>,
    advance: f32,
}

pub struct TextAtlas {
    faces: [FontFace; 3],
    texture: Texture,
    pixels: Vec<u8>,
    glyphs: HashMap<(Font, char, u32), Glyph>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// rows of the atlas changed since the last upload
    dirty: Option<(u32, u32)>,
    scale: f32,
}

fn face_index(font: Font) -> usize {
    match font {
        Font::Regular => 0,
        Font::SemiBold => 1,
        Font::Bold => 2,
    }
}

impl TextAtlas {
    pub fn new(gl: &glow::Context) -> Result<TextAtlas> {
        let load = |bytes: &[u8]| FontFace::from_bytes(bytes, FontSettings::default()).map_err(|e| anyhow!(e));
        let faces = [
            load(include_bytes!("../../../../assets/fonts/Inter-Regular.ttf"))?,
            load(include_bytes!("../../../../assets/fonts/Inter-SemiBold.ttf"))?,
            load(include_bytes!("../../../../assets/fonts/Inter-ExtraBold.ttf"))?,
        ];
        let pixels = vec![0u8; (ATLAS * ATLAS * 4) as usize];
        let texture = unsafe { super::upload(gl, ATLAS, ATLAS, &pixels)? };
        Ok(TextAtlas {
            faces,
            texture,
            pixels,
            glyphs: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
            dirty: None,
            scale: 1.0,
        })
    }

    pub fn texture(&self) -> Texture {
        self.texture
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() > 0.001 {
            self.scale = scale;
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.glyphs.clear();
        self.pixels.iter_mut().for_each(|p| *p = 0);
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.dirty = Some((0, ATLAS));
    }

    fn px(&self, size: f32) -> u32 {
        (size * self.scale).round().max(1.0) as u32
    }

    pub fn ascent(&self, font: Font, size: f32) -> f32 {
        let px = self.px(size) as f32;
        self.faces[face_index(font)]
            .horizontal_line_metrics(px)
            .map(|m| m.ascent / self.scale)
            .unwrap_or(size * 0.8)
    }

    fn glyph(&mut self, font: Font, ch: char, size: f32) -> Glyph {
        let px = self.px(size);
        if let Some(glyph) = self.glyphs.get(&(font, ch, px)) {
            return *glyph;
        }
        let (metrics, coverage) = self.faces[face_index(font)].rasterize(ch, px as f32);
        let advance = metrics.advance_width / self.scale;
        let mut cell = None;
        if metrics.width > 0 && metrics.height > 0 {
            let (w, h) = (metrics.width as u32, metrics.height as u32);
            if self.shelf_x + w + PAD > ATLAS {
                self.shelf_x = 0;
                self.shelf_y += self.shelf_h + PAD;
                self.shelf_h = 0;
            }
            if self.shelf_y + h + PAD > ATLAS {
                // full: start over (rare; glyphs come back as they're drawn again)
                self.reset();
            }
            let (x0, y0) = (self.shelf_x, self.shelf_y);
            for row in 0..h {
                for col in 0..w {
                    let alpha = coverage[(row * w + col) as usize];
                    let at = (((y0 + row) * ATLAS + x0 + col) * 4) as usize;
                    self.pixels[at..at + 4].copy_from_slice(&[255, 255, 255, alpha]);
                }
            }
            self.shelf_x += w + PAD;
            self.shelf_h = self.shelf_h.max(h);
            self.dirty = Some(match self.dirty {
                Some((from, to)) => (from.min(y0), to.max(y0 + h)),
                None => (y0, y0 + h),
            });
            let atlas = ATLAS as f32;
            cell = Some(Cell {
                left: metrics.xmin as f32 / self.scale,
                top: (metrics.ymin as f32 + h as f32) / self.scale,
                width: w as f32 / self.scale,
                height: h as f32 / self.scale,
                uv: [x0 as f32 / atlas, y0 as f32 / atlas, (x0 + w) as f32 / atlas, (y0 + h) as f32 / atlas],
            });
        }
        let glyph = Glyph { cell, advance };
        self.glyphs.insert((font, ch, px), glyph);
        glyph
    }

    fn kern(&self, font: Font, size: f32, left: char, right: char) -> f32 {
        let px = self.px(size) as f32;
        self.faces[face_index(font)].horizontal_kern(left, right, px).unwrap_or(0.0) / self.scale
    }

    /// Glyph positions for one line, cut with "…" when it's wider than `max_width`.
    pub fn layout_line(&mut self, font: Font, size: f32, text: &str, max_width: Option<f32>) -> Vec<Placed> {
        let mut placed = Vec::with_capacity(text.len());
        let mut x = 0.0;
        let mut previous: Option<char> = None;
        for ch in text.chars() {
            if let Some(prev) = previous {
                x += self.kern(font, size, prev, ch);
            }
            let glyph = self.glyph(font, ch, size);
            placed.push(Placed { x, advance: glyph.advance, cell: glyph.cell });
            x += glyph.advance;
            previous = Some(ch);
        }
        if let Some(max) = max_width
            && x > max
        {
            let ellipsis = self.glyph(font, '…', size);
            while let Some(last) = placed.last() {
                if last.x + ellipsis.advance <= max {
                    break;
                }
                placed.pop();
            }
            let at = placed.last().map(|p| p.x + p.advance).unwrap_or(0.0);
            placed.push(Placed { x: at, advance: ellipsis.advance, cell: ellipsis.cell });
        }
        placed
    }

    pub fn measure(&mut self, font: Font, size: f32, text: &str) -> f32 {
        self.layout_line(font, size, text, None).last().map(|p| p.x + p.advance).unwrap_or(0.0)
    }

    /// Greedy word wrap; the last allowed line keeps the rest (and gets cut with an ellipsis when drawn).
    pub fn wrap(&mut self, font: Font, size: f32, text: &str, max_width: f32, max_lines: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        let words: Vec<&str> = text.split_whitespace().collect();
        for (index, word) in words.iter().enumerate() {
            let candidate = if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
            if current.is_empty() || self.measure(font, size, &candidate) <= max_width {
                current = candidate;
                continue;
            }
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines.saturating_sub(1).max(1) && max_lines > 1 {
                current = words[index..].join(" ");
                break;
            }
            current = word.to_string();
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines.truncate(max_lines.max(1));
        lines
    }

    /// Sends the glyphs rasterised since the last call to the GPU.
    pub fn upload_pending(&mut self, gl: &glow::Context) {
        let Some((from, to)) = self.dirty.take() else { return };
        let to = to.min(ATLAS);
        let start = (from * ATLAS * 4) as usize;
        let end = (to * ATLAS * 4) as usize;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture.id));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                from as i32,
                ATLAS as i32,
                (to - from) as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&self.pixels[start..end])),
            );
        }
    }
}
