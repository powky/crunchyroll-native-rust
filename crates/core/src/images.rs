//! Choosing a picture size: the smallest one at least as wide as it will be drawn, so cards stay sharp on the TV's
//! 4K panel without decoding full-size artwork for every tile.

use crunchyroll_rs::common::Image;

/// The smallest picture at least about `min_width` wide (a tenth narrower will do), or the biggest there is: the
/// TV then shows it nearly 1:1, with nothing to shrink.
pub fn pick(images: &[Image], min_width: u32) -> Option<String> {
    let mut sorted: Vec<&Image> = images.iter().collect();
    sorted.sort_by_key(|image| image.width);
    sorted
        .iter()
        .find(|image| image.width as u32 * 10 >= min_width * 9)
        .or_else(|| sorted.last())
        .map(|image| image.source.clone())
}

/// The same pick from the API's JSON: `images` holds lists (sometimes nested once) of `{width, height, source}`,
/// tried in the order of `keys`.
pub fn pick_json(images: Option<&serde_json::Value>, keys: &[&str], min_width: u32) -> Option<String> {
    let images = images?;
    for key in keys {
        let Some(list) = images.get(key).and_then(|list| list.as_array()) else { continue };
        let list = match list.first() {
            Some(serde_json::Value::Array(inner)) => inner,
            _ => list,
        };
        let mut sized: Vec<(u64, &str)> = list
            .iter()
            .filter_map(|image| {
                let source = image.get("source")?.as_str()?;
                Some((image.get("width").and_then(|w| w.as_u64()).unwrap_or(0), source))
            })
            .collect();
        if sized.is_empty() {
            continue;
        }
        sized.sort_by_key(|(width, _)| *width);
        let found = sized.iter().find(|(width, _)| *width * 10 >= min_width as u64 * 9).or(sized.last());
        return found.map(|(_, source)| source.to_string());
    }
    None
}

/// A keyart link from `images_localized` (an imgsrv template), with imgsrv's options (`fit=cover,width=1920`,
/// and the format: JPEG decodes several times faster than WebP on the TV, WebP keeps a logo's transparency).
pub fn localized(item: &serde_json::Value, key: &str, params: &str) -> Option<String> {
    let image = item.get("images_localized")?.get(key)?;
    let image = image.as_array().and_then(|list| list.first()).unwrap_or(image);
    let url = image.get("url")?.as_str()?;
    Some(url.replace("{:param}", params))
}
