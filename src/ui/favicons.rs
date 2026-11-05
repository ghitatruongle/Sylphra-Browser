use std::collections::HashMap;
use std::sync::Arc;

use crate::image_loader::ImageData;

const MAX_FAVICON_ORIGINS: usize = 256;
const MAX_FAVICON_EDGE_PX: u32 = 64;

#[derive(Debug, Clone)]
pub struct FaviconStore {
    entries: HashMap<String, Arc<ImageData>>,
    last_access: HashMap<String, std::time::Instant>,
}

impl Default for FaviconStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FaviconStore {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            last_access: HashMap::new(),
        }
    }

    pub fn get(&self, origin: &str) -> Option<Arc<ImageData>> {
        let key = favicon_origin_key(origin)?;
        self.entries.get(&key).cloned()
    }

    pub fn contains(&self, origin: &str) -> bool {
        favicon_origin_key(origin).is_some_and(|key| self.entries.contains_key(&key))
    }

    pub fn insert(&mut self, origin: &str, data: ImageData) {
        let Some(key) = favicon_origin_key(origin) else {
            return;
        };
        if self.entries.len() >= MAX_FAVICON_ORIGINS && !self.entries.contains_key(&key) {
            if let Some(oldest) = self
                .last_access
                .iter()
                .filter(|(candidate, _)| self.entries.contains_key(*candidate))
                .min_by_key(|(_, instant)| **instant)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
                self.last_access.remove(&oldest);
            }
        }
        self.last_access
            .insert(key.clone(), std::time::Instant::now());
        self.entries.insert(key, Arc::new(data));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_access.clear();
    }
}

pub fn favicon_origin_key(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{host}", parsed.scheme()))
}

pub fn favicon_url_for(origin_url: &str) -> Option<String> {
    let key = favicon_origin_key(origin_url)?;
    Some(format!("{key}/favicon.ico"))
}

pub fn fetch_favicon_sync(origin_url: &str) -> Option<ImageData> {
    let url = favicon_url_for(origin_url)?;
    crate::image_loader::fetch_and_decode_image(&url)
        .ok()
        .map(|data| decoded_favicon_pixels(&data))
}

pub fn decoded_favicon_pixels(data: &ImageData) -> ImageData {
    let scale = 1.0_f32.max(data.width.max(data.height) as f32 / MAX_FAVICON_EDGE_PX.max(1) as f32);
    if scale <= 1.0 {
        return data.clone();
    }
    let target_width = ((data.width as f32 / scale).round() as u32).max(1);
    let target_height = ((data.height as f32 / scale).round() as u32).max(1);
    let mut rgba = Vec::with_capacity(target_width as usize * target_height as usize * 4);
    for y in 0..target_height {
        let source_y = (y as u64 * data.height as u64 / target_height as u64) as u32;
        for x in 0..target_width {
            let source_x = (x as u64 * data.width as u64 / target_width as u64) as u32;
            let offset = (source_y as usize * data.width as usize + source_x as usize) * 4;
            let end = (offset + 4).min(data.rgba_pixels.len());
            rgba.extend_from_slice(&data.rgba_pixels[offset..end]);
        }
    }
    ImageData {
        url: data.url.clone(),
        width: target_width,
        height: target_height,
        rgba_pixels: rgba,
        format: data.format,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_pixel(color: [u8; 4]) -> ImageData {
        ImageData {
            url: "favicon".to_string(),
            width: 1,
            height: 1,
            rgba_pixels: color.to_vec(),
            format: crate::image_loader::ImageFormat::Png,
        }
    }

    #[test]
    fn origin_keys_reject_non_http_schemes() {
        assert!(favicon_origin_key("https://example.com/page").is_some());
        assert!(favicon_origin_key("ftp://example.com").is_none());
        assert!(favicon_origin_key("not a url").is_none());
    }

    #[test]
    fn store_is_bounded_lru() {
        let mut store = FaviconStore::new();
        store.insert("https://a.com", one_pixel([1, 2, 3, 255]));
        assert!(store.contains("https://a.com/x"));
        assert_eq!(store.get("https://a.com").unwrap().width, 1);
        assert!(store.get("https://b.com").is_none());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn favicon_url_points_to_host_root() {
        assert_eq!(
            favicon_url_for("https://Example.com/a/b"),
            Some("https://example.com/favicon.ico".to_string())
        );
    }

    #[test]
    fn oversized_icons_scale_down() {
        let mut data = one_pixel([9, 9, 9, 255]);
        data.width = 128;
        data.height = 128;
        data.rgba_pixels = vec![9; 128 * 128 * 4];
        let scaled = decoded_favicon_pixels(&data);
        assert_eq!(scaled.width, 64);
        assert_eq!(scaled.height, 64);
        assert_eq!(scaled.rgba_pixels.len(), 64 * 64 * 4);
    }
}
