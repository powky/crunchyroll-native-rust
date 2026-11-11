//! Pictures on the GPU: asked of the worker the first time a frame wants one (decoded at the screen's own size, so a
//! 4K TV gets twice the pixels), uploaded when they arrive, and the least recently drawn dropped past a budget in
//! bytes (a 4K backdrop alone is 33 MB).

use std::collections::{HashMap, VecDeque};

use crate::app::worker::{Command, Worker};
use crate::gfx::{Renderer, Texture};

/// GPU memory pictures may use at 1080p; scaled with the screen's pixel count.
const BUDGET_1080P: usize = 120 * 1024 * 1024;
/// Bytes uploaded per frame at most: a big picture costs a frame's time to upload, so they take turns.
const UPLOAD_PER_FRAME: usize = 12 * 1024 * 1024;

enum Slot {
    Pending,
    Ready { texture: Texture, used: u64 },
    Failed,
}

pub struct Images {
    slots: HashMap<String, Slot>,
    frame: u64,
    /// screen pixels per UI pixel
    scale: f32,
    /// arrived, waiting their turn to upload
    queue: VecDeque<(String, u32, u32, Vec<u8>)>,
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

    /// Starts a frame: uploads what arrived, as much as fits in a frame.
    pub fn next_frame(&mut self, gfx: &mut Renderer) {
        self.frame += 1;
        let mut uploaded = 0;
        while let Some((_, w, h, _)) = self.queue.front() {
            let bytes = (*w as usize) * (*h as usize) * 4;
            if uploaded > 0 && uploaded + bytes > UPLOAD_PER_FRAME {
                break;
            }
            let (url, w, h, rgba) = self.queue.pop_front().expect("front");
            uploaded += bytes;
            match gfx.upload(w, h, &rgba) {
                Ok(texture) => {
                    self.slots.insert(url, Slot::Ready { texture, used: self.frame });
                }
                Err(error) => {
                    eprintln!("crunchy: upload: {error:#}");
                    self.slots.insert(url, Slot::Failed);
                }
            }
        }
        if uploaded > 0 {
            self.evict(gfx);
        }
    }

    /// Screen pixels for a width in UI pixels.
    fn px(&self, width: u32) -> u32 {
        (width as f32 * self.scale).round() as u32
    }

    /// The picture if it's loaded; asks for it otherwise (`width` in UI pixels).
    pub fn get(&mut self, worker: &Worker, url: &str, width: u32) -> Option<Texture> {
        match self.slots.get_mut(url) {
            Some(Slot::Ready { texture, used }) => {
                *used = self.frame;
                Some(*texture)
            }
            Some(_) => None,
            None => {
                self.slots.insert(url.to_string(), Slot::Pending);
                worker.send(Command::Image { url: url.to_string(), width: self.px(width), soon: true });
                None
            }
        }
    }

    /// Asks for a picture ahead of drawing it.
    pub fn prefetch(&mut self, worker: &Worker, url: &str, width: u32) {
        if !self.slots.contains_key(url) {
            self.slots.insert(url.to_string(), Slot::Pending);
            worker.send(Command::Image { url: url.to_string(), width: self.px(width), soon: false });
        }
    }

    pub fn arrived(&mut self, url: String, width: u32, height: u32, rgba: Vec<u8>) {
        self.queue.push_back((url, width, height, rgba));
    }

    /// Whether a picture couldn't be had.
    pub fn is_failed(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Failed))
    }

    pub fn failed(&mut self, url: String) {
        self.slots.insert(url, Slot::Failed);
    }

    fn evict(&mut self, gfx: &mut Renderer) {
        let budget = (BUDGET_1080P as f32 * self.scale * self.scale) as usize;
        let mut ready: Vec<(u64, usize, String)> = self
            .slots
            .iter()
            .filter_map(|(url, slot)| match slot {
                Slot::Ready { used, texture } => Some((*used, texture.width as usize * texture.height as usize * 4, url.clone())),
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
