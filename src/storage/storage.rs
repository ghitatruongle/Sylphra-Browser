use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    #[serde(default)]
    pub same_site: String,
    #[serde(default)]
    pub created_at: i64,
}

impl PartialEq for Cookie {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.domain == other.domain && self.path == other.path
    }
}

impl Eq for Cookie {}

impl std::hash::Hash for Cookie {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.domain.hash(state);
        self.path.hash(state);
    }
}

impl Cookie {
    pub fn new(name: &str, value: &str, domain: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
            domain: domain.to_string(),
            path: path.to_string(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: "lax".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn from_set_cookie_header(header: &str, default_domain: &str) -> Self {
        let mut name = String::new();
        let mut value = String::new();
        let mut domain = default_domain.to_string();
        let mut path = "/".to_string();
        let mut expires: Option<i64> = None;
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = "lax".to_string();

        let parts: Vec<&str> = header.split(';').collect();
        for (i, part) in parts.iter().enumerate() {
            let trimmed = part.trim();
            if let Some(eq_pos) = trimmed.find('=') {
                let key_raw = trimmed[..eq_pos].trim();
                let key = key_raw.to_ascii_lowercase();
                let val = trimmed[eq_pos + 1..].trim().to_string();

                match key.as_str() {
                    "domain" => {
                        let d = val.trim_start_matches('.').to_ascii_lowercase();
                        if is_domain_suffix(&d, default_domain) {
                            domain = format!(".{}", d);
                        }
                    }

                    "path" => {
                        path = if val.starts_with('/') && !val.is_empty() {
                            val
                        } else {
                            "/".to_string()
                        }
                    }
                    "expires" => {
                        if let Some(ts) = parse_http_date(&val) {
                            expires = Some(ts);
                        }
                    }
                    "max-age" => {
                        if let Ok(secs) = val.parse::<i64>() {
                            if secs <= 0 {
                                expires = Some(0);
                            } else {
                                let now = chrono::Utc::now().timestamp();
                                let capped = secs.min(100 * 365 * 24 * 60 * 60);
                                expires = Some(now.saturating_add(capped));
                            }
                        }
                    }
                    "samesite" => same_site = val.to_lowercase(),
                    _ => {
                        if i == 0 && name.is_empty() {
                            name = key_raw.to_string();
                            value = val;
                        }
                    }
                }
            } else {
                let trimmed_lower = trimmed.to_lowercase();
                if trimmed_lower == "secure" {
                    secure = true;
                }
                if trimmed_lower == "httponly" {
                    http_only = true;
                }
            }
        }

        Self {
            name,
            value,
            domain,
            path,
            expires,
            secure,
            http_only,
            same_site,
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn matches_url(&self, url: &str) -> bool {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return false,
        };

        if self.secure && parsed.scheme() != "https" {
            return false;
        }

        let base = self.domain.trim_start_matches('.');
        let host = parsed.host_str().unwrap_or("");
        let base = base.to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        let domain_matches = host == base || host.ends_with(&format!(".{}", base));
        if !domain_matches {
            return false;
        }

        if self.same_site.eq_ignore_ascii_case("strict")
            && registrable_domain(&host) != registrable_domain(&base)
        {
            return false;
        }

        let request_path = parsed.path();
        let request_path = if request_path.is_empty() {
            "/"
        } else {
            request_path
        };
        let cookie_path = if self.path.is_empty() || !self.path.starts_with('/') {
            "/"
        } else {
            self.path.as_str()
        };
        request_path == cookie_path
            || (request_path.starts_with(cookie_path)
                && (cookie_path.ends_with('/')
                    || request_path[cookie_path.len()..].starts_with('/')))
    }

    pub fn is_expired(&self) -> bool {
        match self.expires {
            Some(exp) => exp < chrono::Utc::now().timestamp(),
            None => false,
        }
    }

    pub fn to_header_value(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

fn is_domain_suffix(domain: &str, host: &str) -> bool {
    if domain.is_empty() {
        return false;
    }
    let domain = domain.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    if domain != host && !host.ends_with(&format!(".{}", domain)) {
        return false;
    }
    !crate::public_suffix::is_public_suffix(&domain)
}

fn registrable_domain(host: &str) -> String {
    crate::public_suffix::registrable_domain(host).unwrap_or_else(|| host.to_string())
}

pub(crate) fn parse_http_date(s: &str) -> Option<i64> {
    let s = s.trim();

    for fmt in &[
        "%a, %d %b %Y %H:%M:%S GMT",
        "%a, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp());
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieStore {
    cookies: HashMap<String, HashSet<Cookie>>,
}

impl Default for CookieStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieStore {
    pub fn new() -> Self {
        Self {
            cookies: HashMap::new(),
        }
    }

    pub fn add_cookie(&mut self, cookie: Cookie) {
        const MAX_COOKIES_PER_DOMAIN: usize = 180;

        const MAX_TOTAL_COOKIES: usize = 3_000;
        if cookie.name.is_empty() {
            return;
        }
        let domain = cookie.domain.clone();
        let replacing = self
            .cookies
            .get(&domain)
            .is_some_and(|set| set.contains(&cookie));
        if !replacing {
            let total: usize = self.cookies.values().map(|set| set.len()).sum();
            if total >= MAX_TOTAL_COOKIES {
                let mut oldest: Option<(String, Cookie)> = None;
                for (d, set) in &self.cookies {
                    for candidate in set {
                        if oldest
                            .as_ref()
                            .is_none_or(|(_, o)| candidate.created_at < o.created_at)
                        {
                            oldest = Some((d.clone(), candidate.clone()));
                        }
                    }
                }
                if let Some((d, o)) = oldest {
                    if let Some(set) = self.cookies.get_mut(&d) {
                        set.remove(&o);
                    }
                }
            }
        }
        let entry = self.cookies.entry(domain.clone()).or_default();
        if !entry.contains(&cookie) && entry.len() >= MAX_COOKIES_PER_DOMAIN {
            warn!(
                "Dropping cookie {} for {}: per-domain limit ({}) reached",
                cookie.name, domain, MAX_COOKIES_PER_DOMAIN
            );
            return;
        }

        entry.remove(&cookie);
        entry.insert(cookie);
    }

    pub fn get_cookies(&self, domain: &str) -> Vec<Cookie> {
        let mut result = Vec::new();

        for (stored_domain, cookies) in &self.cookies {
            let matches = if stored_domain == domain {
                true
            } else if let Some(base) = stored_domain.strip_prefix('.') {
                base.contains('.') && (domain == base || domain.ends_with(stored_domain))
            } else {
                false
            };

            if matches {
                for cookie in cookies {
                    if !cookie.is_expired() {
                        result.push(cookie.clone());
                    }
                }
            }
        }

        if result.is_empty() && !domain.is_empty() {
            log::debug!("No cookies found for domain: {}", domain);
        }

        result
    }

    pub fn get_cookies_for_url(&self, url: &str) -> Vec<Cookie> {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(_) => return Vec::new(),
        };
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        if host.is_empty() {
            return Vec::new();
        }
        self.get_cookies(&host)
            .into_iter()
            .filter(|c| c.matches_url(url))
            .collect()
    }

    pub fn remove_domain_cookies(&mut self, domain: &str) {
        let base = domain.trim_start_matches('.');
        let variants = [base.to_string(), format!(".{}", base)];
        self.cookies.retain(|key, _| !variants.contains(key));
    }

    pub fn clear_all(&mut self) {
        self.cookies.clear();
    }

    pub fn len(&self) -> usize {
        self.cookies.values().map(|s| s.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn clean_expired(&mut self) {
        let now = chrono::Utc::now().timestamp();
        for cookies in self.cookies.values_mut() {
            cookies.retain(|c| match c.expires {
                Some(exp) => exp > now,
                None => true,
            });
        }
        self.cookies.retain(|_, v| !v.is_empty());
    }
}

pub fn cookie_header_for(store: &CookieStore, url: &str) -> String {
    store
        .get_cookies_for_url(url)
        .iter()
        .map(|c| c.to_header_value())
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalStorage {
    data: HashMap<String, String>,
    origin: String,
    #[serde(default)]
    created_at: i64,
}

const LOCAL_STORAGE_QUOTA: usize = 5 * 1024 * 1024;

impl LocalStorage {
    pub fn new(origin: &str) -> Self {
        Self {
            data: HashMap::new(),
            origin: origin.to_string(),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    pub fn total_bytes(&self) -> usize {
        self.data.iter().map(|(k, v)| k.len() + v.len()).sum()
    }

    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let projected = match self.data.get(key) {
            Some(existing) => self.total_bytes() + value.len().saturating_sub(existing.len()),
            None => self.total_bytes() + key.len() + value.len(),
        };
        if projected > LOCAL_STORAGE_QUOTA {
            warn!(
                "localStorage quota exceeded for origin {} ({} bytes)",
                self.origin, LOCAL_STORAGE_QUOTA
            );
            return false;
        }
        self.data.insert(key.to_string(), value.to_string());
        true
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.data.remove(key)
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.data.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub added_at: i64,
}

fn default_visit_count() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub url: String,
    pub title: String,
    pub visited_at: i64,
    #[serde(default = "default_visit_count")]
    pub visit_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub url: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub completed_at: i64,
    pub success: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserSession {
    pub active_index: usize,
    pub tabs: Vec<SessionTab>,
    pub groups: Vec<SessionTabGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    pub muted: bool,
    pub group_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTabGroup {
    pub id: u64,
    pub name: String,
    pub color: String,
    pub collapsed: bool,
}

fn default_true() -> bool {
    true
}

fn default_memory_saver_threshold_minutes() -> u32 {
    5
}

fn default_memory_soft_limit_mb() -> u32 {
    400
}

fn default_memory_hard_limit_mb() -> u32 {
    500
}

fn default_image_cache_capacity_mb() -> u32 {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSettings {
    pub theme: String,

    pub search_engine: String,

    pub homepage: String,

    pub show_bookmarks_bar: bool,

    pub default_zoom: u16,

    #[serde(default = "default_true")]
    pub pixel_rendering: bool,

    #[serde(default)]
    pub vertical_tabs: bool,

    #[serde(default = "default_true")]
    pub adblock_enabled: bool,

    #[serde(default)]
    pub adblock_disabled_domains: HashSet<String>,

    #[serde(default = "default_true")]
    pub adblock_cosmetic_filtering: bool,

    #[serde(default = "default_true")]
    pub tab_memory_saver: bool,

    #[serde(default = "default_memory_saver_threshold_minutes")]
    pub memory_saver_threshold_minutes: u32,

    #[serde(default = "default_memory_soft_limit_mb")]
    pub memory_soft_limit_mb: u32,

    #[serde(default = "default_memory_hard_limit_mb")]
    pub memory_pressure_threshold_mb: u32,

    #[serde(default = "default_image_cache_capacity_mb")]
    pub image_cache_capacity_mb: u32,

    #[serde(default)]
    pub custom_wallpaper_url: Option<String>,

    #[serde(default)]
    pub youtube_live_playback_enabled: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            search_engine: "google".to_string(),
            homepage: "sylphra://newtab".to_string(),
            show_bookmarks_bar: true,
            default_zoom: 100,
            pixel_rendering: true,
            vertical_tabs: false,
            adblock_enabled: true,
            adblock_disabled_domains: HashSet::new(),
            adblock_cosmetic_filtering: true,
            tab_memory_saver: true,
            memory_saver_threshold_minutes: 5,
            memory_soft_limit_mb: 400,
            memory_pressure_threshold_mb: 500,
            image_cache_capacity_mb: 24,
            custom_wallpaper_url: None,
            youtube_live_playback_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageState {
    #[serde(default = "legacy_storage_schema")]
    schema_version: u32,
    version: String,
    cookies: HashMap<String, HashSet<Cookie>>,
    local_storage: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    #[serde(default = "default_bookmark_root")]
    bookmark_tree: crate::bookmarks::BookmarkItem,
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(default)]
    downloads: Vec<DownloadRecord>,
    #[serde(default)]
    settings: BrowserSettings,
    #[serde(default)]
    session: BrowserSession,
    #[serde(default)]
    permissions: crate::permissions::PermissionStore,
}

const STORAGE_SCHEMA_VERSION: u32 = 3;

fn default_bookmark_root() -> crate::bookmarks::BookmarkItem {
    crate::bookmarks::BookmarksManager::new().root
}

fn legacy_storage_schema() -> u32 {
    1
}

pub struct StorageManager {
    cookies: CookieStore,
    local_storage: HashMap<String, LocalStorage>,
    bookmarks: Vec<Bookmark>,
    bookmark_tree: crate::bookmarks::BookmarksManager,

    history: Vec<HistoryRecord>,

    downloads: Vec<DownloadRecord>,

    pub settings: BrowserSettings,
    storage_dir: Option<PathBuf>,
    session: BrowserSession,
    permissions: crate::permissions::PermissionStore,
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageManager {
    pub fn new() -> Self {
        let storage_dir = if cfg!(test) {
            None
        } else {
            dirs::data_local_dir()
                .map(|p| p.join("Sylphra"))
                .or_else(|| Some(PathBuf::from("./.sylphra_data")))
        };

        Self::from_storage_dir(storage_dir, true)
    }

    pub fn in_memory() -> Self {
        Self::from_storage_dir(None, false)
    }

    fn from_storage_dir(storage_dir: Option<PathBuf>, load_existing: bool) -> Self {
        let mut mgr = Self {
            cookies: CookieStore::new(),
            local_storage: HashMap::new(),
            bookmarks: Vec::new(),
            bookmark_tree: crate::bookmarks::BookmarksManager::new(),
            history: Vec::new(),
            downloads: Vec::new(),
            settings: BrowserSettings::default(),
            storage_dir,
            session: BrowserSession::default(),
            permissions: crate::permissions::PermissionStore::new(),
        };

        if load_existing {
            mgr.load();
        }
        mgr.cookies.clean_expired();

        mgr
    }

    pub fn local_storage(&mut self, origin: &str) -> &mut LocalStorage {
        self.local_storage
            .entry(origin.to_string())
            .or_insert_with(|| LocalStorage::new(origin))
    }

    pub fn get_local_storage(&self, origin: &str) -> Option<&LocalStorage> {
        self.local_storage.get(origin)
    }

    pub fn cookies(&self) -> &CookieStore {
        &self.cookies
    }

    pub fn cookies_mut(&mut self) -> &mut CookieStore {
        &mut self.cookies
    }

    fn storage_path(&self) -> Option<PathBuf> {
        self.storage_dir.as_ref().map(|d| d.join("storage.json"))
    }

    pub fn save(&self) {
        let path = match self.storage_path() {
            Some(p) => p,
            None => {
                warn!("No storage directory available, skipping save");
                return;
            }
        };

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                error!("Failed to create storage directory: {}", e);
                return;
            }
        }

        let ls_map: HashMap<String, HashMap<String, String>> = self
            .local_storage
            .iter()
            .map(|(origin, ls)| (origin.clone(), ls.data.clone()))
            .collect();

        let persisted_cookies: HashMap<String, HashSet<Cookie>> = self
            .cookies
            .cookies
            .iter()
            .map(|(domain, jar)| {
                let persistent: HashSet<Cookie> = jar
                    .iter()
                    .filter(|cookie| cookie.expires.is_some())
                    .cloned()
                    .collect();
                (domain.clone(), persistent)
            })
            .filter(|(_, jar)| !jar.is_empty())
            .collect();

        let state = StorageState {
            schema_version: STORAGE_SCHEMA_VERSION,
            version: crate::VERSION.to_string(),
            cookies: persisted_cookies,
            local_storage: ls_map,
            bookmarks: self.bookmarks.clone(),
            bookmark_tree: self.bookmark_tree.root.clone(),
            history: self.history.clone(),
            downloads: self.downloads.clone(),
            settings: self.settings.clone(),
            session: self.session.clone(),
            permissions: self.permissions.clone(),
        };

        match serde_json::to_string_pretty(&state) {
            Ok(json) => {
                let temporary = path.with_extension("json.tmp");
                let backup = path.with_extension("json.bak");
                let write_result = (|| -> std::io::Result<()> {
                    use std::io::Write;
                    let mut file = std::fs::File::create(&temporary)?;
                    file.write_all(json.as_bytes())?;
                    file.sync_all()?;

                    if path.exists() {
                        if backup.exists() {
                            std::fs::remove_file(&backup)?;
                        }
                        std::fs::rename(&path, &backup)?;
                    }

                    if let Err(error) = std::fs::rename(&temporary, &path) {
                        if backup.exists() && !path.exists() {
                            let _ = std::fs::rename(&backup, &path);
                        }
                        return Err(error);
                    }
                    Ok(())
                })();

                match write_result {
                    Ok(()) => info!("Storage saved to {:?}", path),
                    Err(e) => {
                        let _ = std::fs::remove_file(&temporary);
                        error!("Failed to save storage transactionally: {}", e);
                    }
                }
            }
            Err(e) => error!("Failed to serialize storage: {}", e),
        }
    }

    fn read_state(path: &std::path::Path) -> Option<StorageState> {
        use std::io::Read;
        const MAX_STORAGE_BYTES: u64 = 64 * 1024 * 1024;
        let file = std::fs::File::open(path).ok()?;
        let mut limited = file.take(MAX_STORAGE_BYTES + 1);
        let mut buf = Vec::new();
        limited.read_to_end(&mut buf).ok()?;
        if buf.len() as u64 > MAX_STORAGE_BYTES {
            log::error!("Storage file exceeds 64 MiB bound: {:?}", path);
            return None;
        }
        let state = serde_json::from_slice::<StorageState>(&buf).ok()?;
        (state.schema_version <= STORAGE_SCHEMA_VERSION).then_some(state)
    }

    fn apply_state(&mut self, state: StorageState) {
        self.cookies.cookies = state.cookies;
        self.cookies.clean_expired();

        for (origin, data) in state.local_storage {
            let mut ls = LocalStorage::new(&origin);
            ls.data = data;
            self.local_storage.insert(origin, ls);
        }

        self.bookmarks = state.bookmarks;
        self.bookmark_tree = crate::bookmarks::BookmarksManager::from_root(state.bookmark_tree)
            .unwrap_or_else(|_| crate::bookmarks::BookmarksManager::new());
        if self.bookmark_tree.root.children.is_empty() {
            for bookmark in &self.bookmarks {
                let _ = self.bookmark_tree.add_bookmark(
                    self.bookmark_tree.root.id,
                    &bookmark.title,
                    &bookmark.url,
                );
            }
        }
        self.history = state.history;
        self.downloads = state.downloads;
        self.settings = state.settings;
        self.session = state.session;
        self.permissions = state.permissions;
    }

    pub fn load(&mut self) {
        let path = match self.storage_path() {
            Some(p) => p,
            None => return,
        };

        if !path.exists() {
            let backup = path.with_extension("json.bak");
            if backup.exists() {
                match Self::read_state(&backup) {
                    Some(state) => {
                        self.apply_state(state);
                        info!("Storage restored from backup {:?}", backup);
                        return;
                    }
                    None => warn!("Backup at {:?} was unreadable", backup),
                }
            }
            info!("No saved storage found at {:?}", path);
            return;
        }

        match Self::read_state(&path) {
            Some(state) => {
                self.apply_state(state);
                info!("Storage loaded from {:?}", path);
            }
            None => {
                error!("Failed to parse storage file: {:?}", path);

                let backup = path.with_extension("json.bak");
                if backup.exists() {
                    info!("Attempting to load from backup: {:?}", backup);
                    match Self::read_state(&backup) {
                        Some(state) => {
                            self.apply_state(state);
                            info!("Storage recovered from backup");
                        }
                        None => error!("Backup is also corrupt; keeping in-memory defaults"),
                    }
                }
            }
        }
    }

    pub fn storage_dir(&self) -> Option<&PathBuf> {
        self.storage_dir.as_ref()
    }

    pub fn for_profile(base_dir: impl AsRef<std::path::Path>, name: &str) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 64
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_')
            })
        {
            return Err("Profile name contains unsupported characters".to_string());
        }
        Ok(Self::from_storage_dir(
            Some(base_dir.as_ref().join("Profiles").join(name)),
            true,
        ))
    }

    pub fn session(&self) -> &BrowserSession {
        &self.session
    }

    pub fn set_session(&mut self, session: BrowserSession) {
        self.session = session;
    }

    pub fn permissions(&self) -> &crate::permissions::PermissionStore {
        &self.permissions
    }

    pub fn set_permission(
        &mut self,
        origin: &str,
        permission: crate::permissions::PermissionType,
        state: crate::permissions::PermissionState,
    ) -> Result<(), String> {
        self.permissions.set_permission(origin, permission, state)?;
        self.save();
        Ok(())
    }

    pub fn reset_permissions_for_origin(&mut self, origin: &str) -> bool {
        let reset = self.permissions.reset_origin(origin);
        if reset {
            self.save();
        }
        reset
    }

    pub fn cookie_count(&self) -> usize {
        self.cookies.len()
    }

    pub fn local_storage_count(&self) -> usize {
        self.local_storage.values().map(|ls| ls.len()).sum()
    }

    pub fn local_storage_origins(&self) -> Vec<String> {
        self.local_storage.keys().cloned().collect()
    }

    pub fn clear_local_storage(&mut self) {
        self.local_storage.clear();
        self.save();
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn bookmark_tree(&self) -> &crate::bookmarks::BookmarkItem {
        &self.bookmark_tree.root
    }

    pub fn create_bookmark_folder(&mut self, parent_id: u64, title: &str) -> Result<u64, String> {
        let id = self.bookmark_tree.create_folder(parent_id, title)?;
        self.save();
        Ok(id)
    }

    pub fn add_bookmark_to_folder(
        &mut self,
        parent_id: u64,
        title: &str,
        url: &str,
    ) -> Result<u64, String> {
        let id = self.bookmark_tree.add_bookmark(parent_id, title, url)?;
        if !self.bookmarks.iter().any(|bookmark| bookmark.url == url) {
            self.bookmarks.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
                added_at: chrono::Utc::now().timestamp(),
            });
        }
        self.save();
        Ok(id)
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        if self.is_bookmarked(url) {
            self.bookmarks.retain(|b| b.url != url);
            self.bookmark_tree.remove_url(url);
            false
        } else {
            self.bookmarks.push(Bookmark {
                url: url.to_string(),
                title: if title.trim().is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                added_at: chrono::Utc::now().timestamp(),
            });
            let _ = self
                .bookmark_tree
                .add_bookmark(self.bookmark_tree.root.id, title, url);
            true
        }
    }

    pub fn remove_bookmark(&mut self, url: &str) {
        self.bookmarks.retain(|b| b.url != url);
        self.bookmark_tree.remove_url(url);
    }

    pub fn history(&self) -> &[HistoryRecord] {
        &self.history
    }

    pub fn add_history(&mut self, url: &str, title: &str) {
        let now = chrono::Utc::now().timestamp();
        let prev_count = if let Some(pos) = self.history.iter().position(|h| h.url == url) {
            let old = self.history.remove(pos);
            old.visit_count
        } else {
            0
        };
        self.history.insert(
            0,
            HistoryRecord {
                url: url.to_string(),
                title: if title.trim().is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                visited_at: now,
                visit_count: prev_count + 1,
            },
        );
        self.history.truncate(2000);
    }

    pub fn remove_history_entry(&mut self, url: &str) {
        self.history.retain(|h| h.url != url);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn top_sites(&self, n: usize) -> Vec<HistoryRecord> {
        let mut sorted: Vec<HistoryRecord> = self.history.clone();
        sorted.sort_by(|a, b| {
            b.visit_count
                .cmp(&a.visit_count)
                .then(b.visited_at.cmp(&a.visited_at))
        });
        sorted.truncate(n);
        sorted
    }

    pub fn downloads(&self) -> &[DownloadRecord] {
        &self.downloads
    }

    pub fn add_download(&mut self, record: DownloadRecord) {
        self.downloads.insert(0, record);
        self.downloads.truncate(200);
    }

    pub fn clear_downloads(&mut self) {
        self.downloads.clear();
    }
}

impl Drop for StorageManager {
    fn drop(&mut self) {
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_creation() {
        let cookie = Cookie::new("session", "abc123", ".example.com", "/");
        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc123");
        assert_eq!(cookie.domain, ".example.com");
        assert!(!cookie.is_expired());
    }

    #[test]
    fn missing_memory_fields_use_personal_defaults() {
        let defaults = BrowserSettings::default();
        let mut value = serde_json::to_value(defaults).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("memory_saver_threshold_minutes");
        object.remove("memory_soft_limit_mb");
        object.remove("memory_pressure_threshold_mb");
        object.remove("image_cache_capacity_mb");
        let restored: BrowserSettings = serde_json::from_value(value).unwrap();
        assert_eq!(restored.memory_saver_threshold_minutes, 5);
        assert_eq!(restored.memory_soft_limit_mb, 400);
        assert_eq!(restored.memory_pressure_threshold_mb, 500);
        assert_eq!(restored.image_cache_capacity_mb, 24);
    }

    #[test]
    fn test_cookie_store() {
        let mut store = CookieStore::new();
        let cookie = Cookie::new("test", "value", ".example.com", "/");
        store.add_cookie(cookie);
        assert_eq!(store.len(), 1);

        let cookies = store.get_cookies(".example.com");
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "test");
    }

    #[test]
    fn test_cookie_expiry() {
        let mut store = CookieStore::new();
        let mut cookie = Cookie::new("expired", "old", ".test.com", "/");
        cookie.expires = Some(0);
        store.add_cookie(cookie);

        assert!(store.get_cookies(".test.com").is_empty());
    }

    #[test]
    fn test_local_storage_basics() {
        let mut ls = LocalStorage::new("https://example.com");
        assert!(ls.set("key1", "value1"));
        assert!(ls.set("key2", "value2"));
        assert_eq!(ls.len(), 2);
        assert_eq!(ls.get("key1"), Some(&"value1".to_string()));

        ls.remove("key1");
        assert_eq!(ls.len(), 1);

        ls.clear();
        assert!(ls.is_empty());
    }

    #[test]
    fn test_local_storage_quota() {
        let mut ls = LocalStorage::new("https://example.com");

        let chunk = "x".repeat(LOCAL_STORAGE_QUOTA - 10);
        assert!(ls.set("big", &chunk));

        assert!(!ls.set("more", "data"), "write over quota must be refused");
        assert!(ls.get("more").is_none());

        assert!(ls.set("big", "small"));
    }

    #[test]
    fn test_storage_manager() {
        let mut mgr = StorageManager::new();
        {
            let ls = mgr.local_storage("https://example.com");
            ls.set("theme", "dark");
            ls.set("font_size", "14");
        }
        assert_eq!(mgr.local_storage_count(), 2);

        let cookie = Cookie::new("session", "xyz", ".example.com", "/");
        mgr.cookies_mut().add_cookie(cookie);
        assert_eq!(mgr.cookie_count(), 1);
    }

    #[test]
    fn test_cookie_domain_matching() {
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("a", "1", ".example.com", "/"));
        store.add_cookie(Cookie::new("b", "2", "example.com", "/"));

        assert_eq!(store.get_cookies("example.com").len(), 2);
        assert_eq!(store.get_cookies("sub.example.com").len(), 1);
    }

    #[test]
    fn test_cookie_secure_flag() {
        let mut cookie = Cookie::new("secure_cookie", "secret", ".bank.com", "/");
        cookie.secure = true;
        assert!(cookie.secure);
        assert_eq!(cookie.to_header_value(), "secure_cookie=secret");
    }

    #[test]
    fn test_cookie_rejects_foreign_domain() {
        let cookie = Cookie::from_set_cookie_header(
            "session=abc; Domain=attacker.com; Path=/",
            "example.com",
        );
        assert_eq!(cookie.domain, "example.com");
        assert!(!cookie.matches_url("https://attacker.com/"));
        assert!(cookie.matches_url("https://example.com/"));
    }

    #[test]
    fn test_cookie_accepts_domain_suffix() {
        let cookie = Cookie::from_set_cookie_header("x=1; Domain=example.com", "sub.example.com");
        assert_eq!(cookie.domain, ".example.com");
    }

    #[test]
    fn test_cookie_rejects_bare_suffix() {
        let cookie = Cookie::from_set_cookie_header("x=1; Domain=com", "example.com");
        assert_eq!(cookie.domain, "example.com");
    }

    #[test]
    fn test_cookie_parses_expires_date() {
        let cookie = Cookie::from_set_cookie_header(
            "x=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
            "example.com",
        );
        assert_eq!(cookie.expires, Some(1_445_412_480));
    }

    #[test]
    fn test_cookie_invalid_expires_ignored() {
        let cookie = Cookie::from_set_cookie_header("x=1; Expires=not-a-date", "example.com");
        assert_eq!(cookie.expires, None);
    }

    #[test]
    fn test_cookie_max_age_zero_deletes() {
        let cookie = Cookie::from_set_cookie_header("x=1; Max-Age=0", "example.com");
        assert_eq!(cookie.expires, Some(0));
        assert!(cookie.is_expired());
    }

    #[test]
    fn test_matches_url_label_boundary() {
        let cookie = Cookie::new("a", "1", ".example.com", "/");
        assert!(cookie.matches_url("https://example.com/"));
        assert!(cookie.matches_url("https://sub.example.com/x"));
        assert!(!cookie.matches_url("https://badexample.com/"));
        assert!(!cookie.matches_url("https://notexample.com.evil.com/"));
        assert!(!cookie.matches_url("not a url"));
    }

    #[test]
    fn test_matches_url_secure_only_https() {
        let mut cookie = Cookie::new("s", "1", ".bank.com", "/");
        cookie.secure = true;
        assert!(cookie.matches_url("https://bank.com/"));
        assert!(!cookie.matches_url("http://bank.com/"));
    }

    #[test]
    fn test_matches_url_path_prefix() {
        let cookie = Cookie::new("a", "1", ".example.com", "/app");
        assert!(cookie.matches_url("https://example.com/app"));
        assert!(cookie.matches_url("https://example.com/app/page"));
        assert!(!cookie.matches_url("https://example.com/appx"));
    }

    #[test]
    fn test_same_site_strict_not_sent_cross_site() {
        let mut cookie = Cookie::new("s", "1", "example.com", "/");
        cookie.same_site = "strict".to_string();

        assert!(cookie.matches_url("https://example.com/page"));
        assert!(cookie.matches_url("https://www.example.com/page"));

        assert!(!cookie.matches_url("https://badexample.com/page"));

        let lax = Cookie::new("l", "1", "example.com", "/");
        assert!(lax.matches_url("https://www.example.com/page"));
    }

    #[test]
    fn test_max_age_overflow_is_capped() {
        let cookie =
            Cookie::from_set_cookie_header("k=v; Max-Age=9223372036854775807", "example.com");
        assert!(
            cookie
                .expires
                .is_some_and(|ts| ts > chrono::Utc::now().timestamp()),
            "huge Max-Age must produce a far-future expiry, not overflow"
        );
    }

    #[test]
    fn test_cookie_domain_limit_drops_new_cookies() {
        let mut store = CookieStore::new();

        let mut overflow_dropped = false;
        for i in 0..200 {
            let before = store.len();
            store.add_cookie(Cookie::new(&format!("c{}", i), "1", "example.com", "/"));
            if store.len() == before {
                overflow_dropped = true;
            }
        }
        assert!(overflow_dropped, "extra cookies must be dropped at the cap");
        assert!(store.len() <= 180);
    }

    #[test]
    fn test_remove_domain_cookies_dot_variants() {
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("a", "1", ".example.com", "/"));
        store.add_cookie(Cookie::new("b", "2", "example.com", "/"));
        assert_eq!(store.len(), 2);

        store.remove_domain_cookies("example.com");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_cookies_rejects_public_suffix() {
        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("evil", "1", ".com", "/"));
        assert!(store.get_cookies("victim.com").is_empty());
        assert!(store.get_cookies("example.com").is_empty());
    }

    #[test]
    fn test_backup_recovery() {
        let dir = std::env::temp_dir().join(format!("sylphra_baktest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("storage.json");
        let backup = dir.join("storage.json.bak");

        let mut store = CookieStore::new();
        store.add_cookie(Cookie::new("sid", "abc", ".example.com", "/"));
        let state = StorageState {
            schema_version: STORAGE_SCHEMA_VERSION,
            version: crate::VERSION.to_string(),
            cookies: store.cookies,
            local_storage: HashMap::new(),
            bookmarks: Vec::new(),
            bookmark_tree: default_bookmark_root(),
            history: Vec::new(),
            downloads: Vec::new(),
            settings: BrowserSettings::default(),
            session: BrowserSession::default(),
            permissions: crate::permissions::PermissionStore::new(),
        };
        std::fs::write(&backup, serde_json::to_string_pretty(&state).unwrap()).unwrap();

        std::fs::write(&path, "{corrupt").unwrap();

        let mut mgr = StorageManager::new();
        mgr.storage_dir = Some(dir.clone());
        mgr.load();

        assert_eq!(mgr.cookie_count(), 1);
        mgr.storage_dir = None;
        drop(mgr);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_transactional_save_has_no_temporary_file_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("sylphra_txn_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut manager = StorageManager::new();
        manager.storage_dir = Some(dir.clone());

        let mut persistent = Cookie::new("session", "value", "example.com", "/");
        persistent.expires = Some(chrono::Utc::now().timestamp() + 3600);
        manager.cookies_mut().add_cookie(persistent);

        manager
            .cookies_mut()
            .add_cookie(Cookie::new("ephemeral", "value", "example.com", "/"));
        manager
            .local_storage("https://example.com")
            .set("key", "value");
        manager.save();

        assert!(dir.join("storage.json").exists());
        assert!(!dir.join("storage.json.tmp").exists());

        let mut restored = StorageManager::new();
        restored.storage_dir = Some(dir.clone());
        restored.load();
        assert_eq!(restored.cookie_count(), 1);
        assert_eq!(restored.local_storage_count(), 1);

        restored.clear_local_storage();
        assert_eq!(restored.local_storage_count(), 0);
        manager.storage_dir = None;
        restored.storage_dir = None;
        drop((manager, restored));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
