//! Pictures on the GPU: asked of the worker the first time a frame wants one (decoded at the screen's own size, so a
//! 4K TV gets twice the pixels), uploaded a few rows a frame once they arrive (a 4K backdrop is 33 MB: in one go it
//! stalled a frame for a third of a second), and the least recently drawn dropped past a budget in bytes.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::app::worker::{Command, Worker};
use crate::gfx::{PixelFormat, Renderer, Texture, UploadJob};

/// GPU memory pictures may use at 1080p; at 4K it doubles (not 4 times: the TV has 700 MB free, shared with the
/// video).
const BUDGET_1080P: usize = 120 * 1024 * 1024;
/// Bytes uploaded per frame at most: pictures go up in strips across frames. The TV's driver takes ~13 ms a MB (it
/// rearranges the pixels on the CPU); that time overlaps the wait for the GPU as long as it stays under it.
const UPLOAD_PER_FRAME: usize = 768 * 1024;

/// A picture that couldn't be had is asked for again this long after (twice as long each time), a few times: most
/// failures are the network's.
const RETRY_AFTER: Duration = Duration::from_secs(2);
const RETRIES: u8 = 3;

enum Slot {
    /// asked for (`tries` times before)
    Pending { tries: u8 },
    Ready { texture: Texture, used: u64, bytes: usize },
    Failed { at: Instant, tries: u8 },
}

/// A picture going up to the GPU: its texture once made, and the rows done.
struct Upload {
    url: String,
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: Vec<u8>,
    texture: Option<Texture>,
    row: u32,
}

pub struct Images {
    slots: HashMap<String, Slot>,
    frame: u64,
    /// screen pixels per UI pixel
    scale: f32,
    /// arrived, going up in turn
    queue: VecDeque<Upload>,
}

impl Default for Images {
    fn default() -> Images {
        Images { slots: HashMap::new(), frame: 0, scale: 1.0, queue: VecDeque::new() }
    }
}

impl Images {
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// Starts a frame: what arrived goes to the upload thread (and what it made is ready), or else up a few rows.
    pub fn next_frame(&mut self, gfx: &mut Renderer) {
        self.frame += 1;
        let started = Instant::now();
        let mut finished = false;
        if let Some(uploads) = &gfx.uploads {
            while self.queue.front().is_some_and(|upload| upload.texture.is_none()) {
                let upload = self.queue.pop_front().expect("front");
                let Upload { url, width, height, format, pixels, .. } = upload;
                uploads.send(UploadJob { key: url, width, height, format, pixels });
            }
            for (url, texture, bytes) in uploads.finished() {
                match texture {
                    Ok(texture) => {
                        self.slots.insert(url, Slot::Ready { texture, used: self.frame, bytes });
                        finished = true;
                        gfx.invalidate();
                    }
                    Err(error) => {
                        eprintln!("crunchy: upload: {error:#}");
                        self.slots.insert(url, Slot::Failed { at: Instant::now(), tries: RETRIES });
                    }
                }
            }
        }
        let mut budget = if gfx.uploads.is_some() { 0 } else { UPLOAD_PER_FRAME };
        while budget > 0
            && let Some(upload) = self.queue.front_mut()
        {
            let texture = match upload.texture {
                Some(texture) => texture,
                None => match gfx.create_texture(upload.width, upload.height, upload.format) {
                    Ok(texture) => *upload.texture.insert(texture),
                    Err(error) => {
                        eprintln!("crunchy: upload: {error:#}");
                        let upload = self.queue.pop_front().expect("front");
                        self.slots.insert(upload.url, Slot::Failed { at: Instant::now(), tries: RETRIES });
                        continue;
                    }
                },
            };
            let row_bytes = upload.width as usize * upload.format.bytes();
            let rows = (budget / row_bytes).max(1).min((upload.height - upload.row) as usize) as u32;
            let from = upload.row as usize * row_bytes;
            let strip = &upload.pixels[from..from + rows as usize * row_bytes];
            gfx.upload_rows(&texture, upload.row, rows, upload.format, strip);
            upload.row += rows;
            budget = budget.saturating_sub(rows as usize * row_bytes);
            if upload.row >= upload.height {
                let upload = self.queue.pop_front().expect("front");
                let bytes = upload.pixels.len();
                self.slots.insert(upload.url, Slot::Ready { texture, used: self.frame, bytes });
                finished = true;
            }
        }
        if finished {
            self.evict(gfx);
        }
        let took = started.elapsed();
        if took > Duration::from_millis(12) {
            eprintln!("crunchy: uploads took {took:?} this frame");
        }
    }

    /// Screen pixels for a width in UI pixels.
    fn px(&self, width: u32) -> u32 {
        (width as f32 * self.scale).round() as u32
    }

    /// The picture if it's loaded; asks for it otherwise (`width` in UI pixels).
    pub fn get(&mut self, worker: &Worker, url: &str, width: u32) -> Option<Texture> {
        if let Some(Slot::Ready { texture, used, .. }) = self.slots.get_mut(url) {
            *used = self.frame;
            return Some(*texture);
        }
        self.ask(worker, url, width, true);
        None
    }

    /// Asks for a picture ahead of drawing it.
    pub fn prefetch(&mut self, worker: &Worker, url: &str, width: u32) {
        self.ask(worker, url, width, false);
    }

    /// Sends for a picture not asked for yet, or failed a while ago (and not too many times).
    fn ask(&mut self, worker: &Worker, url: &str, width: u32, soon: bool) {
        let tries = match self.slots.get(url) {
            None => 0,
            Some(Slot::Failed { at, tries }) if *tries < RETRIES && at.elapsed() >= RETRY_AFTER * (1 << *tries) => *tries + 1,
            Some(_) => return,
        };
        self.slots.insert(url.to_string(), Slot::Pending { tries });
        worker.send(Command::Image { url: url.to_string(), width: self.px(width), soon });
    }

    /// Whether a picture is on the GPU.
    pub fn is_ready(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Ready { .. }))
    }

    pub fn arrived(&mut self, url: String, width: u32, height: u32, format: PixelFormat, pixels: Vec<u8>) {
        self.queue.push_back(Upload { url, width, height, format, pixels, texture: None, row: 0 });
    }

    /// Whether a picture couldn't be had (for now: it's asked for again a little later).
    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Failed { .. }))
    }

    pub fn failed(&mut self, url: String) {
        let tries = match self.slots.get(&url) {
            Some(Slot::Pending { tries }) => *tries,
            _ => RETRIES,
        };
        self.slots.insert(url, Slot::Failed { at: Instant::now(), tries });
    }

    fn evict(&mut self, gfx: &mut Renderer) {
        let budget = (BUDGET_1080P as f32 * (self.scale * self.scale).min(2.0)) as usize;
        let mut ready: Vec<(u64, usize, String)> = self
            .slots
            .iter()
            .filter_map(|(url, slot)| match slot {
                Slot::Ready { used, bytes, .. } => Some((*used, *bytes, url.clone())),
                _ => None,
            })
            .collect();
        let mut total: usize = ready.iter().map(|(_, bytes, _)| bytes).sum();
        if total <= budget {
            return;
        }
        ready.sort();
        for (used, bytes, url) in ready {
            // what this frame drew stays
            if total <= budget || used >= self.frame - 1 {
                break;
            }
            if let Some(Slot::Ready { texture, .. }) = self.slots.remove(&url) {
                gfx.delete(texture);
                total -= bytes;
            }
        }
    }
}
