//! Choosing a picture size: the smallest one at least as wide as it will be drawn, so cards stay sharp on the TV's
//! 4K panel without decoding full-size artwork for every tile.

use crunchyroll_rs::common::Image;

/// The smallest picture at least `min_width` wide, or the biggest there is.
pub fn pick(images: &[Image], min_width: u32) -> Option<String> {
    let mut sorted: Vec<&Image> = images.iter().collect();
    sorted.sort_by_key(|image| image.width);
    sorted
        .iter()
        .find(|image| image.width >= min_width)
        .or_else(|| sorted.last())
        .map(|image| image.source.clone())
}
