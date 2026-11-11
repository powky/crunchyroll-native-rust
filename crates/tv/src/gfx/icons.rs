//! Icons and the Crunchyroll logo: SVG path data rasterised by tiny-skia into the text atlas the first time each is
//! drawn at a size, then tinted like text. The icons are Apple's SF Symbols as the web app generates them
//! (`assets/icons/sf-symbols.json`, local only: not in git) where that file is, else Google's Material Icons Round
//! (`assets/icons/open-icons.json`, Apache-2.0); see build.rs.

use std::collections::HashMap;
use std::sync::OnceLock;

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Play,
    Plus,
    Check,
    CheckCircle,
    Bubble,
    Grid,
    Note,
    Search,
    Gear,
    Power,
    Sort,
    Star,
    StarOutline,
    ChevronDown,
    ChevronRight,
    Edit,
    Back,
    Xmark,
    TextSize,
    Speaker,
    Photo,
    Rows,
    Sparkles,
    Tv,
    Clock,
    Logout,
    User,
    Trash,
    Film,
    Heart,
    Pause,
    Info,
    /// forward.fill: skip an intro, jump ahead
    Skip,
    Rewind,
    PrevEpisode,
    NextEpisode,
    ThumbsUp,
    ThumbsDown,
    ThumbsUpOutline,
    ThumbsDownOutline,
    /// the Crunchyroll wordmark with its symbol, 303x52
    Logo,
}

impl Icon {
    fn glyph_name(self) -> &'static str {
        match self {
            Icon::Play => "play",
            Icon::Plus => "plus",
            Icon::Check => "check",
            Icon::CheckCircle => "checkCircle",
            Icon::Bubble => "bubble",
            Icon::Grid => "grid",
            Icon::Note => "note",
            Icon::Search => "search",
            Icon::Gear => "gear",
            Icon::Power => "power",
            Icon::Sort => "sort",
            Icon::Star => "star",
            Icon::StarOutline => "starOutline",
            Icon::ChevronDown => "chevronDown",
            Icon::ChevronRight => "chevronRight",
            Icon::Edit => "edit",
            Icon::Back => "back",
            Icon::Xmark => "xmark",
            Icon::TextSize => "textSize",
            Icon::Speaker => "speaker",
            Icon::Photo => "photo",
            Icon::Rows => "rows",
            Icon::Sparkles => "sparkles",
            Icon::Tv => "tv",
            Icon::Clock => "clock",
            Icon::Logout => "logout",
            Icon::User => "user",
            Icon::Trash => "trash",
            Icon::Film => "film",
            Icon::Heart => "heart",
            Icon::Pause => "pause",
            Icon::Info => "info",
            Icon::Skip => "skip",
            Icon::Rewind => "rewind",
            Icon::PrevEpisode => "prevEp",
            Icon::NextEpisode => "nextEp",
            Icon::ThumbsUp => "thumbsUp",
            Icon::ThumbsDown => "thumbsDown",
            Icon::ThumbsUpOutline => "thumbsUpOutline",
            Icon::ThumbsDownOutline => "thumbsDownOutline",
            Icon::Logo => "",
        }
    }

    /// Width over height of what's drawn: icons sit in a square, the logo is wide.
    pub fn aspect(self) -> f32 {
        if self == Icon::Logo { LOGO_W / LOGO_H } else { 1.0 }
    }
}

/// SF Symbols share one 230-unit box (100 pt symbols), so they keep Apple's relative optical sizes.
const SF_BOX: f32 = 230.0;
const LOGO_W: f32 = 303.0;
const LOGO_H: f32 = 52.0;
const LOGO_SVG: &str = include_str!("../../../../assets/icons/crunchyroll-logo.svg");

struct Glyph {
    w: f32,
    h: f32,
    /// the glyph's units per box unit (the SF Symbols are drawn in the box's own units; Material's 24-unit icons are
    /// scaled up to about an SF symbol's size)
    s: f32,
    d: String,
}

fn glyphs() -> &'static HashMap<String, Glyph> {
    static GLYPHS: OnceLock<HashMap<String, Glyph>> = OnceLock::new();
    GLYPHS.get_or_init(|| {
        let json: serde_json::Value =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/icons.json"))).unwrap_or_default();
        let mut out = HashMap::new();
        if let Some(map) = json.get("glyphs").and_then(|glyphs| glyphs.as_object()) {
            for (name, glyph) in map {
                let number = |key: &str| glyph.get(key).and_then(|v| v.as_f64()).map(|v| v as f32);
                if let Some(d) = glyph.get("d").and_then(|d| d.as_str()) {
                    let (w, h, s) = (number("w").unwrap_or(0.0), number("h").unwrap_or(0.0), number("s").unwrap_or(1.0));
                    out.insert(name.clone(), Glyph { w, h, s, d: d.to_string() });
                }
            }
        }
        out
    })
}

/// Coverage (one byte a pixel) for an icon `height` pixels tall, and its width.
pub fn rasterize(icon: Icon, height: u32) -> Option<(u32, u32, Vec<u8>)> {
    let height = height.max(1);
    let (width, d, transform) = if icon == Icon::Logo {
        let scale = height as f32 / LOGO_H;
        let d = LOGO_SVG.split(" d=\"").nth(1)?.split('"').next()?;
        ((LOGO_W * scale).ceil() as u32, d, Transform::from_scale(scale, scale))
    } else {
        let glyph = glyphs().get(icon.glyph_name())?;
        let scale = height as f32 / SF_BOX;
        let transform = Transform::from_row(
            scale * glyph.s,
            0.0,
            0.0,
            scale * glyph.s,
            (SF_BOX - glyph.w * glyph.s) / 2.0 * scale,
            (SF_BOX - glyph.h * glyph.s) / 2.0 * scale,
        );
        (height, glyph.d.as_str(), transform)
    };
    let path = parse_path(d)?;
    let mut pixmap = Pixmap::new(width, height)?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
    let coverage = pixmap.data().chunks_exact(4).map(|pixel| pixel[3]).collect();
    Some((width, height, coverage))
}

/// SVG path data: M L H V C S Q T A Z, absolute and relative.
fn parse_path(d: &str) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut pen = Pen::default();
    let mut numbers: Vec<f32> = Vec::new();
    let mut command = ' ';
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            pen.run_all(command, &numbers, &mut pb);
            numbers.clear();
            command = c;
            if c == 'Z' || c == 'z' {
                pb.close();
                // a closed subpath leaves the pen at its start
                (pen.x, pen.y) = pen.start;
                pen.control = None;
            }
            i += 1;
        } else if matches!(command, 'A' | 'a') && matches!(numbers.len() % 7, 3 | 4) && (c == '0' || c == '1') {
            // an arc's flags are one digit each, often written with nothing between them and what follows
            numbers.push(if c == '1' { 1.0 } else { 0.0 });
            i += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            let begin = i;
            let mut seen_dot = c == '.';
            i += 1;
            while i < bytes.len() {
                let n = bytes[i] as char;
                if n.is_ascii_digit() {
                    i += 1;
                } else if n == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else if (n == 'e' || n == 'E') && i + 1 < bytes.len() {
                    i += 1;
                    if bytes[i] == b'-' || bytes[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            numbers.push(d[begin..i].parse().ok()?);
        } else {
            i += 1;
        }
    }
    pen.run_all(command, &numbers, &mut pb);
    pb.finish()
}

#[derive(Default)]
struct Pen {
    x: f32,
    y: f32,
    start: (f32, f32),
    /// the last control point, which S and T mirror
    control: Option<(f32, f32)>,
}

impl Pen {
    /// One command with all the numbers that followed it (a command can repeat without its letter).
    fn run_all(&mut self, command: char, numbers: &[f32], pb: &mut PathBuilder) {
        let arity = match command.to_ascii_uppercase() {
            'M' | 'L' | 'T' => 2,
            'H' | 'V' => 1,
            'C' => 6,
            'S' | 'Q' => 4,
            'A' => 7,
            _ => return,
        };
        for (index, args) in numbers.chunks_exact(arity).enumerate() {
            // pairs after a moveto's first are linetos
            let current = match command {
                'M' if index > 0 => 'L',
                'm' if index > 0 => 'l',
                other => other,
            };
            self.run(current, args, pb);
        }
    }

    fn run(&mut self, command: char, args: &[f32], pb: &mut PathBuilder) {
        let relative = command.is_ascii_lowercase();
        let (ox, oy) = if relative { (self.x, self.y) } else { (0.0, 0.0) };
        let mirrored = |pen: &Pen| pen.control.map(|(cx, cy)| (2.0 * pen.x - cx, 2.0 * pen.y - cy)).unwrap_or((pen.x, pen.y));
        match command.to_ascii_uppercase() {
            'M' => {
                (self.x, self.y) = (ox + args[0], oy + args[1]);
                self.start = (self.x, self.y);
                pb.move_to(self.x, self.y);
                self.control = None;
            }
            'L' => {
                (self.x, self.y) = (ox + args[0], oy + args[1]);
                pb.line_to(self.x, self.y);
                self.control = None;
            }
            'H' => {
                self.x = ox + args[0];
                pb.line_to(self.x, self.y);
                self.control = None;
            }
            'V' => {
                self.y = oy + args[0];
                pb.line_to(self.x, self.y);
                self.control = None;
            }
            'C' => {
                let (x2, y2) = (ox + args[2], oy + args[3]);
                (self.x, self.y) = (ox + args[4], oy + args[5]);
                pb.cubic_to(ox + args[0], oy + args[1], x2, y2, self.x, self.y);
                self.control = Some((x2, y2));
            }
            'S' => {
                let (x1, y1) = mirrored(self);
                let (x2, y2) = (ox + args[0], oy + args[1]);
                (self.x, self.y) = (ox + args[2], oy + args[3]);
                pb.cubic_to(x1, y1, x2, y2, self.x, self.y);
                self.control = Some((x2, y2));
            }
            'Q' => {
                let (x1, y1) = (ox + args[0], oy + args[1]);
                (self.x, self.y) = (ox + args[2], oy + args[3]);
                pb.quad_to(x1, y1, self.x, self.y);
                self.control = Some((x1, y1));
            }
            'T' => {
                let (x1, y1) = mirrored(self);
                (self.x, self.y) = (ox + args[0], oy + args[1]);
                pb.quad_to(x1, y1, self.x, self.y);
                self.control = Some((x1, y1));
            }
            'A' => {
                let to = (ox + args[5], oy + args[6]);
                arc_to(pb, (self.x, self.y), to, (args[0], args[1]), args[2], args[3] != 0.0, args[4] != 0.0);
                (self.x, self.y) = to;
                self.control = None;
            }
            _ => {}
        }
    }
}

/// An SVG elliptical arc as cubic Béziers of at most a quarter turn each (the SVG spec's endpoint-to-centre
/// conversion, F.6.5).
fn arc_to(pb: &mut PathBuilder, from: (f32, f32), to: (f32, f32), radii: (f32, f32), degrees: f32, large: bool, sweep: bool) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let ((x1, y1), (x2, y2)) = (from, to);
    if (x1 - x2).abs() < 1e-6 && (y1 - y2).abs() < 1e-6 {
        return;
    }
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());
    if rx < 1e-6 || ry < 1e-6 {
        pb.line_to(x2, y2);
        return;
    }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let (x1p, y1p) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    // radii too small to reach are scaled up just enough
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
    let (cx, cy) = (cos * cxp - sin * cyp + (x1 + x2) / 2.0, sin * cxp + cos * cyp + (y1 + y2) / 2.0);
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = ((ux * vx + uy * vy) / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let start = angle(1.0, 0.0, ux, uy);
    let mut delta = angle(ux, uy, vx, vy);
    if !sweep && delta > 0.0 {
        delta -= TAU;
    } else if sweep && delta < 0.0 {
        delta += TAU;
    }
    let segments = (delta.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / segments as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f32| {
        let (s, c) = t.sin_cos();
        (cx + rx * c * cos - ry * s * sin, cy + rx * c * sin + ry * s * cos)
    };
    let slope = |t: f32| {
        let (s, c) = t.sin_cos();
        (-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos)
    };
    let mut t = start;
    for index in 0..segments {
        let next = t + step;
        let ((p1x, p1y), (d1x, d1y)) = (point(t), slope(t));
        let ((mut p2x, mut p2y), (d2x, d2y)) = (point(next), slope(next));
        if index + 1 == segments {
            (p2x, p2y) = (x2, y2);
        }
        pb.cubic_to(p1x + k * d1x, p1y + k * d1y, p2x - k * d2x, p2y - k * d2y, p2x, p2y);
        t = next;
    }
}
