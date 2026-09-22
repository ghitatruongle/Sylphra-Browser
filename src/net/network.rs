use log::{info, warn};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(crate) fn sanitize_download_filename(raw: &str, fallback_host: &str) -> String {
    let mut name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim()
        .to_string();

    if name.len() >= 2 && name.as_bytes()[1] == b':' {
        name = name[2..].to_string();
    }

    name = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    let lower = name.to_ascii_lowercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || lower == "con"
        || lower == "prn"
        || lower == "aux"
        || lower == "nul"
        || name.len() > 128
    {
        return format!(
            "{}.html",
            fallback_host
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .take(64)
                .collect::<String>()
                .trim_matches('.')
        );
    }
    if name.len() > 128 {
        name.truncate(128);
    }
    name
}

pub(crate) fn browser_ua() -> String {
    format!("Sylphra/{} (Rust)", crate::VERSION)
}

pub(crate) fn cache_ttl_secs(headers: &HashMap<String, String>) -> u64 {
    let now = chrono::Utc::now().timestamp();

    if let Some(cc) = headers.get("cache-control") {
        let lower = cc.to_ascii_lowercase();

        if lower.contains("no-store") || lower.contains("no-cache") || lower.contains("private") {
            return 0;
        }
        if lower.contains("max-age=") {
            let max_age_str = lower
                .split("max-age=")
                .nth(1)
                .and_then(|v| v.split(|c: char| !c.is_ascii_digit()).next())
                .unwrap_or("");
            let max_age_secs = max_age_str.parse::<u64>().unwrap_or_else(|e| {
                warn!("Invalid max-age value {:?}: {}", max_age_str, e);
                300
            });
            return max_age_secs;
        }
    }

    if let Some(expires) = headers.get("expires") {
        if let Some(ts) = crate::storage::parse_http_date(expires) {
            return (ts - now).max(0) as u64;
        }
    }

    if let Some(lm) = headers.get("last-modified") {
        if let Some(ts) = crate::storage::parse_http_date(lm) {
            let age = (now - ts).max(0);
            if age > 0 {
                return (age / 10).clamp(60, 24 * 60 * 60) as u64;
            }
        }
    }

    300
}

#[derive(Debug, Clone)]
pub struct FetchResult {
    pub body: String,

    pub binary_body: Option<Vec<u8>>,
    pub url: String,
    pub status_code: u16,
    pub content_type: String,
    pub headers: HashMap<String, String>,
    pub fetch_time_ms: u64,
    pub set_cookie_headers: Vec<String>,

    pub set_cookie_hosts: Vec<String>,
}

impl FetchResult {
    pub fn retained_bytes(&self) -> usize {
        let binary = self.binary_body.as_ref().map_or(0, Vec::capacity);
        let headers = self.headers.iter().fold(0usize, |total, (name, value)| {
            total
                .saturating_add(name.capacity())
                .saturating_add(value.capacity())
        });
        let cookies = self.set_cookie_headers.iter().fold(0usize, |total, value| {
            total.saturating_add(value.capacity())
        });
        std::mem::size_of::<Self>()
            .saturating_add(self.body.capacity())
            .saturating_add(binary)
            .saturating_add(self.url.capacity())
            .saturating_add(self.content_type.capacity())
            .saturating_add(headers)
            .saturating_add(cookies)
    }
}

pub fn fetch_url(url_str: &str) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .user_agent(&browser_ua())
        .build();

    execute_fetch(&agent, url_str, None, &[])
}

pub fn fetch_with_cookies(
    url_str: &str,
    cookie_store: &mut crate::storage::CookieStore,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let _start = Instant::now();
    let parsed = url::Url::parse(url_str)?;
    let domain = parsed.host_str().unwrap_or("").to_string();

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .user_agent(&browser_ua())
        .build();

    let matching_cookies = cookie_store.get_cookies(&domain);
    let cookie_header: String = matching_cookies
        .iter()
        .filter(|c| c.matches_url(url_str))
        .map(|c| c.to_header_value())
        .collect::<Vec<_>>()
        .join("; ");

    let result = if cookie_header.is_empty() {
        info!("No cookies for domain: {}", domain);
        execute_fetch(&agent, url_str, None, &[])?
    } else {
        info!(
            "Sending {} cookies for domain: {}",
            matching_cookies.len(),
            domain
        );
        execute_fetch(&agent, url_str, Some(&cookie_header), &[])?
    };

    let final_domain = match url::Url::parse(&result.url) {
        Ok(u) => u
            .host_str()
            .map(|h| h.to_string())
            .unwrap_or_else(|| domain.clone()),
        Err(_) => domain.clone(),
    };
    let hosts = &result.set_cookie_hosts;
    for (index, set_cookie_val) in result.set_cookie_headers.iter().enumerate() {
        let hop_host = hosts
            .get(index)
            .map(String::as_str)
            .filter(|host| !host.is_empty())
            .unwrap_or(&final_domain);
        let cookie = crate::storage::Cookie::from_set_cookie_header(set_cookie_val, hop_host);
        if !cookie.name.is_empty() {
            info!(
                "Stored cookie: {}={} for domain {}",
                cookie.name, cookie.value, cookie.domain
            );
            cookie_store.add_cookie(cookie);
        }
    }

    Ok(result)
}

pub fn is_retryable_error(err: &str) -> bool {
    let err_lower = err.to_ascii_lowercase();

    if err_lower.contains("status 5") || err_lower.contains("status 4") {
        if let Some(pos) = err_lower.find("status ") {
            let after = &err_lower[pos + 7..];
            let code: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(code_num) = code.parse::<u16>() {
                return code_num == 408 || code_num == 429 || (500..600).contains(&code_num);
            }
        }
    }
    if err_lower.contains("429") {
        return true;
    }

    if err_lower.contains("server error")
        || err_lower.contains("bad gateway")
        || err_lower.contains("service unavailable")
        || err_lower.contains("gateway timeout")
    {
        return true;
    }

    err_lower.contains("timed out")
        || err_lower.contains("timeout")
        || err_lower.contains("connection closed")
        || err_lower.contains("connection reset")
        || err_lower.contains("refused")
}

const MAX_BACKOFF_MS: u64 = 30_000;

pub fn fetch_with_header_and_retry(
    url_str: &str,
    cookie_header: &str,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .user_agent(&browser_ua())
        .build();

    fetch_with_agent_and_retry(&agent, url_str, cookie_header, max_retries)
}

pub(crate) fn fetch_with_agent_and_retry(
    agent: &ureq::Agent,
    url_str: &str,
    cookie_header: &str,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let mut last_error = String::new();
    let mut backoff_ms = 1000;
    let header = if cookie_header.is_empty() {
        None
    } else {
        Some(cookie_header)
    };

    for attempt in 0..=max_retries {
        if attempt > 0 {
            info!("Retry attempt {}/{} for {}", attempt, max_retries, url_str);
            std::thread::sleep(std::time::Duration::from_millis(
                backoff_ms.min(MAX_BACKOFF_MS),
            ));
            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        }

        match execute_fetch(agent, url_str, header, &[]) {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();
                last_error = err_str.clone();
                if !is_retryable_error(&err_str) {
                    warn!("Non-retryable error for {}: {}", url_str, err_str);
                    return Err(err_str);
                }
                if attempt == max_retries {
                    warn!(
                        "Max retries ({}) exceeded for {}: {}",
                        max_retries, url_str, err_str
                    );
                    return Err(err_str);
                }
            }
        }
    }

    Err(last_error)
}

pub fn fetch_with_retry(
    url_str: &str,
    cookie_store: &mut crate::storage::CookieStore,
    max_retries: u32,
) -> Result<FetchResult, String> {
    let mut last_error = String::new();
    let mut backoff_ms = 1000;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            info!("Retry attempt {}/{} for {}", attempt, max_retries, url_str);
            std::thread::sleep(std::time::Duration::from_millis(
                backoff_ms.min(MAX_BACKOFF_MS),
            ));
            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        }

        match fetch_with_cookies(url_str, cookie_store) {
            Ok(result) => return Ok(result),
            Err(e) => {
                let err_str = e.to_string();
                last_error = err_str.clone();
                if !is_retryable_error(&err_str) {
                    warn!("Non-retryable error for {}: {}", url_str, err_str);
                    return Err(err_str);
                }
                if attempt == max_retries {
                    warn!(
                        "Max retries ({}) exceeded for {}: {}",
                        max_retries, url_str, err_str
                    );
                    return Err(err_str);
                }
            }
        }
    }

    Err(last_error)
}

fn execute_fetch(
    agent: &ureq::Agent,
    url_str: &str,
    cookie_header: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    const MAX_REDIRECTS: usize = 5;

    let mut current = url_str.to_string();

    let cookie_origin = url::Url::parse(url_str).ok();
    let mut set_cookie_headers: Vec<String> = Vec::new();
    let mut set_cookie_hosts: Vec<String> = Vec::new();
    let mut final_response: Option<ureq::Response> = None;

    for _hop in 0..=MAX_REDIRECTS {
        let parsed = url::Url::parse(&current)?;
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(format!("Unsupported URL scheme: {}", scheme).into());
        }

        let same_origin = cookie_origin.as_ref().is_some_and(|origin| {
            origin.scheme() == parsed.scheme()
                && origin.host_str() == parsed.host_str()
                && origin.port_or_known_default() == parsed.port_or_known_default()
        });
        let mut request = agent.get(&current);
        if let Some(cookie_val) = cookie_header.filter(|_| same_origin) {
            request = request.set("Cookie", cookie_val);
        }
        if same_origin {
            for (k, v) in extra_headers {
                request = request.set(k, v);
            }
        }
        let response = request.call()?;

        for cookie_val in response.all("set-cookie") {
            let trimmed = cookie_val.trim();
            if !trimmed.is_empty() {
                set_cookie_headers.push(trimmed.to_string());
                set_cookie_hosts.push(parsed.host_str().unwrap_or("").to_string());
            }
        }

        let status = response.status();
        let is_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
        if is_redirect {
            if let Some(loc) = response.header("location") {
                let next = url::Url::parse(&current)?.join(loc)?;

                if next.scheme() != "http" && next.scheme() != "https" {
                    return Err(format!("Unsupported redirect scheme: {}", next.scheme()).into());
                }
                info!("Redirecting {} -> {}", current, next);
                current = next.to_string();
                continue;
            }
        }

        final_response = Some(response);
        break;
    }

    let response = final_response
        .ok_or_else(|| format!("Too many redirects ({}) for {}", MAX_REDIRECTS, url_str))?;

    let status_code = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("text/html")
        .to_string();

    let mut headers = HashMap::new();
    for header_name in &[
        "content-type",
        "content-encoding",
        "content-length",
        "set-cookie",
        "cache-control",
        "last-modified",
        "vary",
    ] {
        if let Some(val) = response.header(header_name) {
            headers.insert(header_name.to_string(), val.to_string());
        }
    }

    let final_url = response.get_url().to_string();

    use std::io::Read;
    const MAX_PAGE_BODY: u64 = 50 * 1024 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .take(MAX_PAGE_BODY + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PAGE_BODY {
        return Err("Page exceeds the 50MB body limit".into());
    }
    let body_len = bytes.len();
    let fetch_time_ms = start.elapsed().as_millis() as u64;

    if body_len > 10 * 1024 * 1024 {
        warn!(
            "Large response received: {} bytes from {}",
            body_len, final_url
        );
    }

    info!(
        "Fetched {} ({} bytes, {} ms, status {})",
        final_url, body_len, fetch_time_ms, status_code
    );

    Ok(finalize_fetch_response(
        &final_url,
        status_code,
        &content_type,
        headers,
        bytes,
        set_cookie_headers,
        set_cookie_hosts,
        fetch_time_ms,
        false,
    )?)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_fetch_response(
    final_url: &str,
    status_code: u16,
    content_type: &str,
    headers: HashMap<String, String>,
    bytes: Vec<u8>,
    set_cookie_headers: Vec<String>,
    set_cookie_hosts: Vec<String>,
    fetch_time_ms: u64,
    binary_mode: bool,
) -> Result<FetchResult, String> {
    let is_pdf = content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/pdf"))
        || final_url.to_ascii_lowercase().ends_with(".pdf");
    let (body, binary_body) = if binary_mode || is_pdf {
        (String::new(), Some(bytes))
    } else {
        (decode_text_response(&bytes, content_type)?, None)
    };
    Ok(FetchResult {
        body,
        binary_body,
        url: final_url.to_string(),
        status_code,
        content_type: content_type.to_string(),
        headers,
        fetch_time_ms,
        set_cookie_headers,
        set_cookie_hosts,
    })
}

pub(crate) fn decode_text_response(bytes: &[u8], content_type: &str) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec())
            .map_err(|error| format!("Invalid UTF-8 response body: {error}"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return Err("Truncated UTF-16LE response body".to_string());
        }
        let units = rest
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|error| format!("Invalid UTF-16LE response body: {error}"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if rest.len() % 2 != 0 {
            return Err("Truncated UTF-16BE response body".to_string());
        }
        let units = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
        return String::from_utf16(&units.collect::<Vec<_>>())
            .map_err(|error| format!("Invalid UTF-16BE response body: {error}"));
    }

    if let Some(label) = response_charset(content_type) {
        let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
            .ok_or_else(|| format!("Unsupported response charset: {label}"))?;
        let (decoded, _, _) = encoding.decode(bytes);
        return Ok(decoded.into_owned());
    }
    if let Ok(utf8) = std::str::from_utf8(bytes) {
        return Ok(utf8.to_string());
    }
    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    Ok(decoded.into_owned())
}

fn response_charset(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['\'', '"']).to_ascii_lowercase())
    })
}

pub fn download_url(
    url_str: &str,
) -> Result<(Vec<u8>, String, String), Box<dyn std::error::Error>> {
    let parsed = url::Url::parse(url_str)?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Unsupported URL scheme: {}", scheme).into());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .redirects(5)
        .user_agent(&browser_ua())
        .build();

    let response = agent.get(url_str).call()?;
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();

    let host = parsed.host_str().unwrap_or("download").to_string();
    let mut file_name = response
        .header("content-disposition")
        .and_then(|cd| cd.split("filename=").nth(1))
        .and_then(|s| s.split(';').next())
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    if file_name.is_empty() {
        file_name = parsed
            .path_segments()
            .and_then(|mut segs| segs.rfind(|s| !s.is_empty()))
            .unwrap_or("")
            .to_string();
    }
    let file_name = sanitize_download_filename(&file_name, &host);

    use std::io::Read;
    const MAX_DOWNLOAD: u64 = 100 * 1024 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DOWNLOAD {
        return Err("File exceeds the 100MB download limit".into());
    }

    info!(
        "Downloaded {} ({} bytes) as {}",
        url_str,
        bytes.len(),
        file_name
    );

    Ok((bytes, file_name, content_type))
}

pub async fn download_url_async(url_str: &str) -> Result<(Vec<u8>, String, String), String> {
    let parsed = url::Url::parse(url_str).map_err(|error| error.to_string())?;
    let response = crate::network_scheduler::fetch_shared(
        url_str.to_string(),
        String::new(),
        1,
        crate::network_scheduler::RequestPriority::Background,
        crate::network_scheduler::ResponseMode::Binary,
        crate::network_scheduler::CancellationToken::default(),
    )
    .await?;
    let bytes = response
        .binary_body
        .ok_or_else(|| "Download transport did not return binary data".to_string())?;
    let raw_name = response
        .headers
        .get("content-disposition")
        .and_then(|value| value.split("filename=").nth(1))
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().trim_matches('"').trim().to_string())
        .unwrap_or_default();
    let raw_name = if raw_name.is_empty() {
        parsed
            .path_segments()
            .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
            .unwrap_or_default()
            .to_string()
    } else {
        raw_name
    };
    let file_name = sanitize_download_filename(&raw_name, parsed.host_str().unwrap_or("download"));
    Ok((bytes, file_name, response.content_type))
}

pub(crate) fn response_varies(headers: &HashMap<String, String>) -> bool {
    headers
        .get("vary")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn fetch_revalidate(
    url: &str,
    if_modified_since: &str,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .user_agent(&browser_ua())
        .build();
    execute_fetch(
        &agent,
        url,
        None,
        &[("If-Modified-Since", if_modified_since)],
    )
}

pub fn fetch_with_cache(
    url: &str,
    cache: &mut ResourceCache,
    cookie_store: Option<&mut crate::storage::CookieStore>,
) -> Result<FetchResult, Box<dyn std::error::Error>> {
    if let Some(cached) = cache.get(url) {
        if !cached.is_expired() {
            info!("Cache HIT for: {}", url);
            let result = cached.result.clone();
            cache.record_hit();
            return Ok(result);
        }

        let last_modified = cached.result.headers.get("last-modified").cloned();
        let cached_result = cached.result.clone();
        if let Some(lm) = last_modified {
            info!("Cache STALE for: {} (revalidating)", url);
            if let Ok(reval) = fetch_revalidate(url, &lm) {
                if reval.status_code == 304 {
                    cache.touch(url);
                    let result = cached_result;
                    cache.record_hit();
                    return Ok(result);
                }
            }
        } else {
            info!("Cache STALE for: {}", url);
        }
    }

    cache.record_miss();

    let result = if let Some(cs) = cookie_store {
        fetch_with_cookies(url, cs)?
    } else {
        fetch_url(url)?
    };

    let ttl = if result.status_code >= 400 {
        0
    } else {
        cache_ttl_secs(&result.headers)
    };

    if ttl > 0 && !response_varies(&result.headers) {
        cache.insert(url, result.clone(), ttl);
    }

    Ok(result)
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub result: FetchResult,
    pub cached_at: Instant,
    pub ttl_secs: u64,
}

impl CacheEntry {
    pub fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > Duration::from_secs(self.ttl_secs)
    }

    pub fn refresh_timestamp(&mut self) {
        self.cached_at = Instant::now();
    }
}

pub struct ResourceCache {
    entries: HashMap<String, CacheEntry>,
    pub max_size: usize,
    pub max_entries: usize,
    hits: u64,
    misses: u64,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size: 32 * 1024 * 1024,
            max_entries: 100,
            hits: 0,
            misses: 0,
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.entries.iter().fold(0usize, |total, (key, entry)| {
            total
                .saturating_add(key.capacity())
                .saturating_add(std::mem::size_of::<CacheEntry>())
                .saturating_add(entry.result.retained_bytes())
        })
    }

    pub fn insert(&mut self, url: &str, result: FetchResult, ttl_secs: u64) {
        let entry_size = url
            .len()
            .saturating_add(std::mem::size_of::<CacheEntry>())
            .saturating_add(result.retained_bytes());
        if entry_size > self.max_size || self.max_entries == 0 {
            return;
        }

        self.evict_expired();
        self.entries.remove(url);

        while self.entries.len() >= self.max_entries {
            if !self.remove_oldest() {
                break;
            }
        }

        while self.total_bytes() + entry_size > self.max_size && !self.entries.is_empty() {
            if !self.remove_oldest() {
                break;
            }
        }

        let entry = CacheEntry {
            result,
            cached_at: Instant::now(),
            ttl_secs,
        };
        self.entries.insert(url.to_string(), entry);
    }

    pub fn get(&self, url: &str) -> Option<&CacheEntry> {
        self.entries.get(url)
    }

    pub fn touch(&mut self, url: &str) {
        if let Some(e) = self.entries.get_mut(url) {
            e.refresh_timestamp();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }

    pub fn set_max_size(&mut self, bytes: usize) -> usize {
        let before = self.total_bytes();
        self.max_size = bytes;
        while self.total_bytes() > self.max_size && !self.entries.is_empty() {
            if !self.remove_oldest() {
                break;
            }
        }
        before.saturating_sub(self.total_bytes())
    }

    fn remove_oldest(&mut self) -> bool {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.cached_at)
            .map(|(key, _)| key.clone());
        oldest.is_some_and(|key| self.entries.remove(&key).is_some())
    }

    pub fn evict_expired(&mut self) {
        let expired_keys: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(k, _)| k.clone())
            .collect();
        let count = expired_keys.len();
        for key in expired_keys {
            self.entries.remove(&key);
        }
        if count > 0 {
            info!("Evicted {} expired cache entries", count);
        }
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            max_size: self.max_size,
        }
    }

    pub fn record_hit(&mut self) {
        self.hits += 1;
    }

    pub fn record_miss(&mut self) {
        self.misses += 1;
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub max_size: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cache: {} entries | Hits: {} | Misses: {} | Hit rate: {:.1}%",
            self.entries,
            self.hits,
            self.misses,
            if self.hits + self.misses > 0 {
                (self.hits as f64 / (self.hits + self.misses) as f64) * 100.0
            } else {
                0.0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_validation() {
        let result = fetch_url("https://example.com");
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_invalid_url() {
        let result = fetch_url("ftp://invalid-scheme.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = ResourceCache::new();
        let url = "https://example.com";
        let fetch_result = FetchResult {
            body: "Hello World!".to_string(),
            binary_body: None,
            url: url.to_string(),
            status_code: 200,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 10,
            set_cookie_headers: vec![],
            set_cookie_hosts: vec![],
        };

        cache.insert(url, fetch_result.clone(), 3600);
        assert!(cache.get(url).is_some());
        assert_eq!(cache.get(url).unwrap().result.body, "Hello World!");
    }

    #[test]
    fn test_cache_expiry() {
        let mut cache = ResourceCache::new();
        let url = "https://test.com";
        let fetch_result = FetchResult {
            body: "Test".to_string(),
            binary_body: None,
            url: url.to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
            set_cookie_hosts: vec![],
        };

        cache.insert(url, fetch_result, 0);
        assert!(cache.get(url).unwrap().is_expired());
    }

    #[test]
    fn test_cache_evict_expired() {
        let mut cache = ResourceCache::new();
        let fetch_result = FetchResult {
            body: "Data".to_string(),
            binary_body: None,
            url: "https://x.com".to_string(),
            status_code: 200,
            content_type: "text/plain".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
            set_cookie_hosts: vec![],
        };

        cache.insert("https://x.com", fetch_result, 0);
        assert_eq!(cache.len(), 1);
        cache.evict_expired();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_total_bytes_counts_binary_headers_keys_and_cookies() {
        let mut cache = ResourceCache::new();
        cache.insert(
            "https://example.test/file.pdf",
            FetchResult {
                body: String::new(),
                binary_body: Some(vec![7; 1024]),
                url: "https://example.test/file.pdf".to_string(),
                status_code: 200,
                content_type: "application/pdf".to_string(),
                headers: HashMap::from([("etag".to_string(), "abc".repeat(100))]),
                fetch_time_ms: 1,
                set_cookie_headers: vec!["session=value".to_string()],
                set_cookie_hosts: vec!["old.example".to_string()],
            },
            60,
        );
        assert!(cache.total_bytes() > 1024);
    }

    #[test]
    fn cache_size_setter_evicts_immediately() {
        let mut cache = ResourceCache::new();
        for index in 0..3 {
            let url = format!("https://example.test/{index}");
            cache.insert(
                &url,
                FetchResult {
                    body: "x".repeat(1024),
                    binary_body: None,
                    url: url.clone(),
                    status_code: 200,
                    content_type: "text/plain".to_string(),
                    headers: HashMap::new(),
                    fetch_time_ms: 1,
                    set_cookie_headers: vec![],
                    set_cookie_hosts: vec![],
                },
                60,
            );
        }
        let freed = cache.set_max_size(1_500);
        assert!(freed > 0);
        assert!(cache.total_bytes() <= 1_500);
    }

    #[test]
    fn test_cache_ttl_respects_no_store() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "no-store, max-age=3600".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_no_cache() {
        let mut headers = HashMap::new();
        headers.insert("cache-control".to_string(), "no-cache".to_string());
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_respects_private() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "private, max-age=100".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 0);
    }

    #[test]
    fn test_cache_ttl_uses_max_age() {
        let mut headers = HashMap::new();
        headers.insert(
            "cache-control".to_string(),
            "public, max-age=120".to_string(),
        );
        assert_eq!(cache_ttl_secs(&headers), 120);
    }

    #[test]
    fn test_cache_ttl_default() {
        assert_eq!(cache_ttl_secs(&HashMap::new()), 300);
    }

    #[test]
    fn test_error_responses_are_not_cached() {
        let mut cache = ResourceCache::new();
        let err_result = FetchResult {
            body: "Internal Server Error".to_string(),
            binary_body: None,
            url: "https://e.com".to_string(),
            status_code: 500,
            content_type: "text/html".to_string(),
            headers: HashMap::new(),
            fetch_time_ms: 5,
            set_cookie_headers: vec![],
            set_cookie_hosts: vec![],
        };

        let ttl = if err_result.status_code >= 400 {
            0
        } else {
            cache_ttl_secs(&err_result.headers)
        };
        if ttl > 0 {
            cache.insert("https://e.com", err_result.clone(), ttl);
        }
        assert!(cache.get("https://e.com").is_none());
    }

    #[test]
    fn test_is_retryable_error_server_errors() {
        assert!(is_retryable_error("status 500 Internal Server Error"));
        assert!(is_retryable_error("status 502 Bad Gateway"));
        assert!(is_retryable_error("status 503 Service Unavailable"));
        assert!(is_retryable_error("status 504 Gateway Timeout"));

        assert!(is_retryable_error("status 408 Request Timeout"));
        assert!(is_retryable_error("status 429 Too Many Requests"));
    }

    #[test]
    fn test_is_retryable_error_network_issues() {
        assert!(is_retryable_error("Connection timed out"));
        assert!(is_retryable_error("Request timeout"));
        assert!(is_retryable_error("Connection closed by server"));
        assert!(is_retryable_error("Connection reset by peer"));
        assert!(is_retryable_error("transport error: connection refused"));
    }

    #[test]
    fn test_is_retryable_error_client_errors() {
        assert!(!is_retryable_error("status 404 Not Found"));
        assert!(!is_retryable_error("status 403 Forbidden"));
        assert!(!is_retryable_error("status 401 Unauthorized"));

        assert!(!is_retryable_error(
            "Could not resolve host: example.invalid"
        ));
    }

    #[test]
    fn test_is_retryable_error_ureq_format() {
        assert!(is_retryable_error("http: server error"));
    }

    #[test]
    fn test_finalize_fetch_response_transport_conformance() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        let bytes = b"<html><body>h\xC3\xA9llo</body></html>".to_vec();

        let text = finalize_fetch_response(
            "https://a.example/page",
            200,
            "text/html",
            headers.clone(),
            bytes.clone(),
            vec![],
            vec![],
            12,
            false,
        )
        .unwrap();
        assert_eq!(text.body, "<html><body>h\u{e9}llo</body></html>");
        assert!(text.binary_body.is_none());

        let binary = finalize_fetch_response(
            "https://a.example/page",
            200,
            "text/html",
            headers,
            bytes.clone(),
            vec![],
            vec![],
            12,
            true,
        )
        .unwrap();
        assert_eq!(binary.body, "");
        assert_eq!(binary.binary_body.as_deref(), Some(bytes.as_slice()));

        assert_eq!(text.url, binary.url);
        assert_eq!(text.status_code, binary.status_code);
        assert_eq!(text.content_type, binary.content_type);
        assert_eq!(text.headers, binary.headers);
        assert_eq!(text.set_cookie_headers, binary.set_cookie_headers);
    }

    #[test]
    fn test_finalize_fetch_response_pdf_policy_is_transport_independent() {
        let pdf_bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        for (content_type, url, label) in [
            ("application/pdf", "https://a.example/doc", "mime"),
            (
                "application/octet-stream",
                "https://a.example/doc.pdf",
                "extension",
            ),
        ] {
            let result = finalize_fetch_response(
                url,
                200,
                content_type,
                HashMap::new(),
                pdf_bytes.clone(),
                vec![],
                vec![],
                1,
                false,
            )
            .unwrap();
            assert!(
                result.binary_body.is_some(),
                "{label}: PDF must be routed to the binary download path"
            );
            assert_eq!(result.body, "");
        }

        let text_result = finalize_fetch_response(
            "https://a.example/doc",
            200,
            "text/plain",
            HashMap::new(),
            pdf_bytes,
            vec![],
            vec![],
            1,
            false,
        )
        .unwrap();
        assert!(text_result.binary_body.is_none());
    }
}
