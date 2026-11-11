//! Pictures on the GPU: asked of the worker the first time a frame wants one, uploaded when it arrives, and the
//! least recently drawn dropped past a budget (each card is ~0.4 MB of texture, a backdrop ~8 MB).

use std::collections::HashMap;

use crate::app::worker::{Command, Worker};
use crate::gfx::{Renderer, Texture};

/// Textures kept: a few rows of cards and the backdrops around the focus.
const BUDGET: usize = 140;

enum Slot {
    Pending,
    Ready { texture: Texture, used: u64 },
    Failed,
}

#[derive(Default)]
pub struct Images {
    slots: HashMap<String, Slot>,
    frame: u64,
}

impl Images {
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    /// The picture if it's loaded; asks for it otherwise.
    pub fn get(&mut self, worker: &Worker, url: &str, width: u32) -> Option<Texture> {
        match self.slots.get_mut(url) {
            Some(Slot::Ready { texture, used }) => {
                *used = self.frame;
                Some(*texture)
            }
            Some(_) => None,
            None => {
                self.slots.insert(url.to_string(), Slot::Pending);
                worker.send(Command::Image(url.to_string(), width));
                None
            }
        }
    }

    /// Asks for a picture ahead of drawing it.
    pub fn prefetch(&mut self, worker: &Worker, url: &str, width: u32) {
        if !self.slots.contains_key(url) {
            self.slots.insert(url.to_string(), Slot::Pending);
            worker.send(Command::Image(url.to_string(), width));
        }
    }

    pub fn arrived(&mut self, gfx: &mut Renderer, url: String, width: u32, height: u32, rgba: &[u8]) {
        match gfx.upload(width, height, rgba) {
            Ok(texture) => {
                self.slots.insert(url, Slot::Ready { texture, used: self.frame });
            }
            Err(error) => {
                eprintln!("crunchy: upload: {error:#}");
                self.slots.insert(url, Slot::Failed);
            }
        }
        self.evict(gfx);
    }

    pub fn failed(&mut self, url: String) {
        self.slots.insert(url, Slot::Failed);
    }

    fn evict(&mut self, gfx: &mut Renderer) {
        let mut ready: Vec<(u64, String)> = self
            .slots
            .iter()
            .filter_map(|(url, slot)| match slot {
                Slot::Ready { used, .. } => Some((*used, url.clone())),
                _ => None,
            })
            .collect();
        if ready.len() <= BUDGET {
            return;
        }
        let over = ready.len() - BUDGET;
        ready.sort();
        for (_, url) in ready.into_iter().take(over) {
            if let Some(Slot::Ready { texture, .. }) = self.slots.remove(&url) {
                gfx.delete(texture);
            }
        }
    }
}
