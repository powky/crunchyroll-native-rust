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
        .find(|image| image.width * 10 >= min_width * 9)
        .or_else(|| sorted.last())
        .map(|image| as_jpeg(&image.source, image.width, image.height))
}

/// A catalog picture (`…/catalog/crunchyroll/{hash}.png`: the API lists thumbnails and posters as PNGs at fixed
/// sizes) from imgsrv as a JPEG of the same size instead: 17-30 KB where the PNG is 200-350 KB, so a row's
/// thumbnails come in a third of a second instead of seconds, and decode in a few milliseconds.
pub fn as_jpeg(url: &str, width: u32, height: u32) -> String {
    match url.find("/catalog/") {
        Some(at) if width > 0 && height > 0 => format!(
            "https://imgsrv.crunchyroll.com/cdn-cgi/image/fit=cover,width={width},height={height},format=jpeg,quality=85{}",
            &url[at..]
        ),
        _ => url.to_string(),
    }
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
        let mut sized: Vec<(u64, u64, &str)> = list
            .iter()
            .filter_map(|image| {
                let source = image.get("source")?.as_str()?;
                let side = |key: &str| image.get(key).and_then(|w| w.as_u64()).unwrap_or(0);
                Some((side("width"), side("height"), source))
            })
            .collect();
        if sized.is_empty() {
            continue;
        }
        sized.sort_by_key(|(width, _, _)| *width);
        let found = sized.iter().find(|(width, _, _)| *width * 10 >= min_width as u64 * 9).or(sized.last());
        return found.map(|(width, height, source)| as_jpeg(source, *width as u32, *height as u32));
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

#[cfg(test)]
mod tests {
    use super::as_jpeg;

    #[test]
    fn catalog_pngs_come_as_jpegs_of_the_same_size() {
        let png = "https://www.crunchyroll.com/imgsrv/display/thumbnail/600x338/catalog/crunchyroll/abc.png";
        assert_eq!(
            as_jpeg(png, 600, 338),
            "https://imgsrv.crunchyroll.com/cdn-cgi/image/fit=cover,width=600,height=338,format=jpeg,quality=85/catalog/crunchyroll/abc.png"
        );
    }

    #[test]
    fn other_links_stay_as_they_are() {
        let keyart = "https://imgsrv.crunchyroll.com/cdn-cgi/image/width=1920/display/keyart/T1-hero_background-ZXX";
        assert_eq!(as_jpeg(keyart, 1920, 1080), keyart);
        assert_eq!(as_jpeg("https://example.com/catalog/x.png", 0, 0), "https://example.com/catalog/x.png");
    }
}
