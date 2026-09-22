use log::warn;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ImageData {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub rgba_pixels: Vec<u8>,
    pub format: ImageFormat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Unknown,
}

impl ImageFormat {
    pub fn from_mime(mime: &str) -> Self {
        match mime {
            "image/png" => ImageFormat::Png,
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Unknown,
        }
    }

    pub fn from_extension(path: &str) -> Self {
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "gif" => ImageFormat::Gif,
            "webp" => ImageFormat::Webp,
            "bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Image {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt_text: String,
    pub loaded: bool,
}

impl Image {
    pub fn new(url: &str, width: u32, height: u32) -> Self {
        Image {
            url: url.to_string(),
            width,
            height,
            alt_text: String::new(),
            loaded: false,
        }
    }

    pub fn with_alt(mut self, alt: &str) -> Self {
        self.alt_text = alt.to_string();
        self
    }
}

pub struct ImageCache {
    inner: HashMap<String, Image>,
    decoded: HashMap<String, Arc<ImageData>>,

    last_access: HashMap<String, Instant>,

    max_cache_size: usize,

    max_decoded_images: usize,

    current_size: usize,

    decoded_count: usize,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    const MAX_DECODED_IMAGES: usize = 100;

    const MAX_METADATA_ENTRIES: usize = 1000;

    pub fn new() -> Self {
        Self::with_capacity(24 * 1024 * 1024)
    }

    pub fn with_capacity(max_cache_size: usize) -> Self {
        ImageCache {
            inner: HashMap::new(),
            decoded: HashMap::new(),
            last_access: HashMap::new(),
            max_cache_size,
            max_decoded_images: Self::MAX_DECODED_IMAGES,
            current_size: 0,
            decoded_count: 0,
        }
    }

    pub fn insert_decoded(&mut self, url: String, data: Arc<ImageData>) {
        let size = data.rgba_pixels.len();

        if let Some(old) = self.decoded.remove(&url) {
            self.current_size = self.current_size.saturating_sub(old.rgba_pixels.len());
            self.decoded_count = self.decoded_count.saturating_sub(1);
        }

        self.evict_for_space(size);

        self.evict_for_count();
        self.current_size += size;
        self.decoded_count += 1;
        self.decoded.insert(url.clone(), data.clone());
        self.last_access.insert(url.clone(), Instant::now());

        if let Some(img) = self.inner.get_mut(&url) {
            img.loaded = true;
            img.width = data.width;
            img.height = data.height;
        } else {
            self.inner
                .insert(url.clone(), Image::new(&url, data.width, data.height));
            if let Some(img) = self.inner.get_mut(&url) {
                img.loaded = true;
            }
        }
        self.prune_metadata();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn add(&mut self, url: String, img: Image) {
        self.inner.insert(url, img);
        self.prune_metadata();
    }

    fn prune_metadata(&mut self) {
        if self.inner.len() > Self::MAX_METADATA_ENTRIES {
            let mut candidates: Vec<String> = self
                .inner
                .keys()
                .filter(|k| !self.decoded.contains_key(*k))
                .cloned()
                .collect();

            candidates.sort_by_key(|k| self.last_access.get(k).copied());
            let overflow = self.inner.len() - Self::MAX_METADATA_ENTRIES;
            for key in candidates.into_iter().take(overflow) {
                self.inner.remove(&key);
                self.last_access.remove(&key);
            }
        }

        self.last_access
            .retain(|k, _| self.inner.contains_key(k) || self.decoded.contains_key(k));
    }

    pub fn get(&self, url: &str) -> Option<&Image> {
        self.inner.get(url)
    }

    pub fn access_image(&mut self, url: &str) -> Option<&Image> {
        if self.inner.contains_key(url) {
            self.last_access.insert(url.to_string(), Instant::now());
            self.inner.get(url)
        } else {
            None
        }
    }

    pub fn touch(&mut self, url: &str) {
        if self.inner.contains_key(url) || self.decoded.contains_key(url) {
            self.last_access.insert(url.to_string(), Instant::now());
        }
    }

    pub fn is_decoded(&self, url: &str) -> bool {
        self.decoded.contains_key(url)
    }

    pub fn get_decoded(&mut self, url: &str) -> Option<Arc<ImageData>> {
        if let Some(data) = self.decoded.get(url) {
            self.last_access.insert(url.to_string(), Instant::now());
            return Some(data.clone());
        }
        None
    }

    pub fn load_image(&mut self, url: &str) -> Option<Arc<ImageData>> {
        if let Some(data) = self.get_decoded(url) {
            return Some(data);
        }

        match self.fetch_and_decode(url) {
            Ok(data) => {
                let size = data.rgba_pixels.len();

                if let Some(old) = self.decoded.get(url) {
                    self.current_size = self.current_size.saturating_sub(old.rgba_pixels.len());
                } else {
                    self.decoded_count += 1;
                }

                self.evict_for_space(size);

                self.evict_for_count();

                let data = Arc::new(data);
                self.current_size += size;
                self.decoded.insert(url.to_string(), data.clone());
                self.last_access.insert(url.to_string(), Instant::now());

                if let Some(img) = self.inner.get_mut(url) {
                    img.loaded = true;
                    img.width = data.width;
                    img.height = data.height;
                } else {
                    self.inner
                        .insert(url.to_string(), Image::new(url, data.width, data.height));
                    if let Some(img) = self.inner.get_mut(url) {
                        img.loaded = true;
                    }
                }
                self.prune_metadata();
                Some(data)
            }
            Err(e) => {
                warn!("Failed to load image {}: {}", url, e);
                None
            }
        }
    }

    fn evict_for_space(&mut self, needed_bytes: usize) {
        while self.current_size + needed_bytes > self.max_cache_size && !self.decoded.is_empty() {
            if let Some(lru_key) = self.find_lru_key() {
                self.evict_entry(&lru_key);
            } else {
                break;
            }
        }
    }

    fn evict_for_count(&mut self) {
        while self.decoded_count > self.max_decoded_images && !self.decoded.is_empty() {
            if let Some(lru_key) = self.find_lru_key() {
                self.evict_entry(&lru_key);
            } else {
                break;
            }
        }
    }

    fn find_lru_key(&self) -> Option<String> {
        self.last_access
            .iter()
            .filter(|(url, _)| self.decoded.contains_key(*url))
            .min_by_key(|(_, time)| *time)
            .map(|(url, _)| url.clone())
    }

    fn evict_entry(&mut self, url: &str) {
        if let Some(old) = self.decoded.remove(url) {
            self.current_size = self.current_size.saturating_sub(old.rgba_pixels.len());
            self.decoded_count = self.decoded_count.saturating_sub(1);
        }
        self.last_access.remove(url);

        if let Some(img) = self.inner.get_mut(url) {
            img.loaded = false;
        }
    }

    pub fn evict_stale(&mut self, max_age: std::time::Duration) -> usize {
        let now = Instant::now();
        let stale_keys: Vec<String> = self
            .last_access
            .iter()
            .filter(|(url, time)| {
                self.decoded.contains_key(*url) && now.duration_since(**time) > max_age
            })
            .map(|(url, _)| url.clone())
            .collect();

        let count = stale_keys.len();
        for key in stale_keys {
            self.evict_entry(&key);
        }
        count
    }

    fn fetch_and_decode(&self, url: &str) -> anyhow::Result<ImageData> {
        fetch_and_decode_image(url)
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.decoded.clear();
        self.last_access.clear();
        self.current_size = 0;
        self.decoded_count = 0;
    }

    pub fn memory_usage(&self) -> usize {
        self.current_size
    }

    pub fn capacity(&self) -> usize {
        self.max_cache_size
    }

    pub fn decoded_count(&self) -> usize {
        self.decoded.len()
    }

    pub fn set_capacity(&mut self, new_capacity: usize) {
        self.max_cache_size = new_capacity;

        while self.current_size > self.max_cache_size && !self.decoded.is_empty() {
            if let Some(lru_key) = self.find_lru_key() {
                self.evict_entry(&lru_key);
            } else {
                break;
            }
        }

        while self.decoded_count > self.max_decoded_images && !self.decoded.is_empty() {
            if let Some(lru_key) = self.find_lru_key() {
                self.evict_entry(&lru_key);
            } else {
                break;
            }
        }
    }
}

pub fn fetch_and_decode_image(url: &str) -> anyhow::Result<ImageData> {
    let bytes = if url.starts_with("http://") || url.starts_with("https://") {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout_read(std::time::Duration::from_secs(30))
            .redirects(5)
            .user_agent(&crate::network::browser_ua())
            .build();
        let response = agent.get(url).call()?;

        const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(anyhow::anyhow!("Image exceeds the 50MB download limit"));
        }
        bytes
    } else if let Some(path) = url.strip_prefix("file://") {
        const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| anyhow::anyhow!("Cannot stat local image: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(anyhow::anyhow!("Refusing to load symlinked image file"));
        }
        if !metadata.is_file() {
            return Err(anyhow::anyhow!("Local image path is not a file"));
        }
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(anyhow::anyhow!("Image exceeds the 50MB download limit"));
        }
        bytes
    } else {
        return Err(anyhow::anyhow!("Unsupported image URL scheme: {}", url));
    };

    decode_image_bytes(url, bytes)
}

pub async fn fetch_and_decode_image_async(url: &str) -> anyhow::Result<ImageData> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return fetch_and_decode_image(url);
    }
    let response = crate::network_scheduler::fetch_shared(
        url.to_string(),
        String::new(),
        1,
        crate::network_scheduler::RequestPriority::Image,
        crate::network_scheduler::ResponseMode::Binary,
        crate::network_scheduler::CancellationToken::default(),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    let bytes = response
        .binary_body
        .ok_or_else(|| anyhow::anyhow!("Image transport did not return binary data"))?;
    let url = url.to_string();
    tokio::task::spawn_blocking(move || decode_image_bytes(&url, bytes))
        .await
        .map_err(|error| anyhow::anyhow!("Image decode task failed: {error}"))?
}

fn decode_image_bytes(url: &str, bytes: Vec<u8>) -> anyhow::Result<ImageData> {
    let dims = image::io::Reader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
    let (w, h) = dims.into_dimensions()?;
    const MAX_IMAGE_DIMENSION: u32 = 8192;

    const MAX_IMAGE_PIXELS: u64 = 16_777_216;
    if w > MAX_IMAGE_DIMENSION
        || h > MAX_IMAGE_DIMENSION
        || (w as u64) * (h as u64) > MAX_IMAGE_PIXELS
    {
        return Err(anyhow::anyhow!(
            "Image dimensions {}x{} exceed the {}px limit",
            w,
            h,
            MAX_IMAGE_DIMENSION
        ));
    }

    let img = image::io::Reader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()?
        .decode()?;
    let img = if img.width() > 800 || img.height() > 800 {
        img.resize(800, 800, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba.into_raw();
    let format = ImageFormat::from_extension(url);

    Ok(ImageData {
        url: url.to_string(),
        width,
        height,
        rgba_pixels: pixels,
        format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_image_cache_budget_is_24_mb() {
        assert_eq!(ImageCache::new().capacity(), 24 * 1024 * 1024);
    }

    #[test]
    fn test_image_creation() {
        let img = Image::new("https://example.com/img.png", 100, 200);
        assert_eq!(img.url, "https://example.com/img.png");
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 200);
    }

    #[test]
    fn test_image_cache_basics() {
        let mut cache = ImageCache::new();
        let img = Image::new("https://example.com/test.png", 50, 50);
        cache.add("https://example.com/test.png".to_string(), img);
        assert_eq!(cache.len(), 1);

        let retrieved = cache.get("https://example.com/test.png");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().width, 50);
    }

    #[test]
    fn test_image_with_alt() {
        let img =
            Image::new("https://example.com/photo.jpg", 800, 600).with_alt("A beautiful landscape");
        assert_eq!(img.alt_text, "A beautiful landscape");
    }

    #[test]
    fn test_format_from_extension() {
        assert_eq!(ImageFormat::from_extension("image.png"), ImageFormat::Png);
        assert_eq!(ImageFormat::from_extension("photo.jpg"), ImageFormat::Jpeg);
        assert_eq!(
            ImageFormat::from_extension("animation.gif"),
            ImageFormat::Gif
        );
    }

    #[test]
    fn test_bare_path_is_not_read_as_file() {
        let mut cache = ImageCache::new();
        assert!(cache.load_image("logo.png").is_none());
        assert!(cache
            .load_image("data:image/png;base64,iVBORw0KGgo=")
            .is_none());
    }

    #[test]
    fn test_local_file_url_requires_valid_image() {
        let mut cache = ImageCache::new();
        assert!(cache.load_image("file:///definitely/missing.png").is_none());
    }

    #[test]
    fn test_cache_capacity_default() {
        let cache = ImageCache::new();
        assert_eq!(cache.capacity(), 24 * 1024 * 1024);
    }

    #[test]
    fn test_cache_custom_capacity() {
        let cache = ImageCache::with_capacity(1024);
        assert_eq!(cache.capacity(), 1024);
    }

    #[test]
    fn test_access_updates_lru() {
        let mut cache = ImageCache::new();
        cache.add(
            "https://a.com/1.png".to_string(),
            Image::new("https://a.com/1.png", 10, 10),
        );
        cache.add(
            "https://a.com/2.png".to_string(),
            Image::new("https://a.com/2.png", 10, 10),
        );

        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.access_image("https://a.com/1.png");

        assert!(
            cache.last_access.contains_key("https://a.com/1.png"),
            "Image 1 should have access time after being accessed"
        );

        assert!(
            !cache.last_access.contains_key("https://a.com/2.png"),
            "Image 2 should not have access time"
        );
    }

    #[test]
    fn test_evict_stale_entries() {
        let mut cache = ImageCache::new();
        cache.add(
            "https://a.com/old.png".to_string(),
            Image::new("https://a.com/old.png", 10, 10),
        );
        cache.add(
            "https://a.com/new.png".to_string(),
            Image::new("https://a.com/new.png", 10, 10),
        );

        let old_data = Arc::new(ImageData {
            url: "https://a.com/old.png".to_string(),
            width: 10,
            height: 10,
            rgba_pixels: vec![0u8; 100],
            format: ImageFormat::Png,
        });
        let new_data = Arc::new(ImageData {
            url: "https://a.com/new.png".to_string(),
            width: 10,
            height: 10,
            rgba_pixels: vec![0u8; 100],
            format: ImageFormat::Png,
        });

        cache
            .decoded
            .insert("https://a.com/old.png".to_string(), old_data);
        cache
            .decoded
            .insert("https://a.com/new.png".to_string(), new_data);
        cache.current_size = 200;

        cache.last_access.insert(
            "https://a.com/old.png".to_string(),
            Instant::now() - std::time::Duration::from_secs(7200),
        );

        cache
            .last_access
            .insert("https://a.com/new.png".to_string(), Instant::now());

        let evicted = cache.evict_stale(std::time::Duration::from_secs(3600));
        assert_eq!(evicted, 1);
        assert!(!cache.decoded.contains_key("https://a.com/old.png"));
        assert!(cache.decoded.contains_key("https://a.com/new.png"));
        assert_eq!(cache.current_size, 100);
    }

    #[test]
    fn test_set_capacity_triggers_eviction() {
        let mut cache = ImageCache::with_capacity(1000);

        let data = Arc::new(ImageData {
            url: "https://a.com/img.png".to_string(),
            width: 10,
            height: 10,
            rgba_pixels: vec![0u8; 800],
            format: ImageFormat::Png,
        });
        cache
            .decoded
            .insert("https://a.com/img.png".to_string(), data.clone());
        cache
            .last_access
            .insert("https://a.com/img.png".to_string(), Instant::now());
        cache.current_size = 800;

        cache.set_capacity(500);

        assert_eq!(cache.decoded_count(), 0);
        assert_eq!(cache.current_size, 0);
    }

    #[test]
    fn test_touch_updates_access_time() {
        let mut cache = ImageCache::new();
        cache.add(
            "https://a.com/img.png".to_string(),
            Image::new("https://a.com/img.png", 10, 10),
        );

        cache.touch("https://a.com/img.png");
        assert!(cache.last_access.contains_key("https://a.com/img.png"));
    }

    #[test]
    fn test_is_decoded() {
        let mut cache = ImageCache::new();
        cache.add(
            "https://a.com/img.png".to_string(),
            Image::new("https://a.com/img.png", 10, 10),
        );

        assert!(!cache.is_decoded("https://a.com/img.png"));

        let data = Arc::new(ImageData {
            url: "https://a.com/img.png".to_string(),
            width: 10,
            height: 10,
            rgba_pixels: vec![0u8; 100],
            format: ImageFormat::Png,
        });
        cache
            .decoded
            .insert("https://a.com/img.png".to_string(), data);
        assert!(cache.is_decoded("https://a.com/img.png"));
    }

    #[test]
    fn test_evict_for_space_lru_order() {
        let mut cache = ImageCache::with_capacity(300);

        for i in 0..3 {
            let url = format!("https://a.com/{}.png", i);
            let data = Arc::new(ImageData {
                url: url.clone(),
                width: 10,
                height: 10,
                rgba_pixels: vec![0u8; 100],
                format: ImageFormat::Png,
            });
            cache.decoded.insert(url.clone(), data);
            cache.last_access.insert(url, Instant::now());
        }
        cache.current_size = 300;

        std::thread::sleep(std::time::Duration::from_millis(10));
        cache.touch("https://a.com/0.png");

        cache.evict_for_space(100);

        assert!(cache.decoded.contains_key("https://a.com/0.png"));

        assert_eq!(cache.decoded_count(), 2);
    }

    #[test]
    fn test_reinsert_same_url_does_not_double_count() {
        let mut cache = ImageCache::with_capacity(10 * 1024 * 1024);

        let make = |url: &str, pixels: usize, w: u32| {
            Arc::new(ImageData {
                url: url.to_string(),
                width: w,
                height: 1,
                rgba_pixels: vec![0u8; pixels],
                format: ImageFormat::Png,
            })
        };

        cache.insert_decoded(
            "https://a.com/img.png".to_string(),
            make("https://a.com/img.png", 100, 10),
        );
        assert_eq!(cache.memory_usage(), 100);
        assert_eq!(cache.decoded_count(), 1);

        cache.insert_decoded(
            "https://a.com/img.png".to_string(),
            make("https://a.com/img.png", 250, 12),
        );
        assert_eq!(
            cache.memory_usage(),
            250,
            "re-insert must replace the entry, not double-count its size"
        );
        assert_eq!(
            cache.decoded_count(),
            1,
            "re-insert must not bump the decoded entry count"
        );

        cache.insert_decoded(
            "https://a.com/2.png".to_string(),
            make("https://a.com/2.png", 50, 5),
        );
        assert_eq!(cache.memory_usage(), 300);
        assert_eq!(cache.decoded_count(), 2);

        assert_eq!(cache.decoded.len(), cache.decoded_count);
    }
}
