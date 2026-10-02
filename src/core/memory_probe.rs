use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PROBE_CACHE_TTL: Duration = Duration::from_millis(250);

const PAGE_BYTES: u64 = 4_096;

const _: () = assert!(PAGE_BYTES >= 1_024);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    Unavailable(String),
}

impl ProbeError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unavailable(detail) => detail,
        }
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ProbeError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SystemMemory {
    pub total_bytes: u64,

    pub available_bytes: u64,

    pub load_percent: u32,
}

impl SystemMemory {
    pub fn has_information(&self) -> bool {
        self.total_bytes > 0
    }

    pub fn available_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            return 1.0;
        }
        self.available_bytes as f64 / self.total_bytes as f64
    }

    pub fn available_mb(&self) -> u64 {
        self.available_bytes / (1024 * 1024)
    }

    pub fn total_mb(&self) -> u64 {
        self.total_bytes / (1024 * 1024)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemorySample {
    pub process_working_set_bytes: u64,

    pub system: SystemMemory,
}

impl MemorySample {
    pub fn has_system_information(&self) -> bool {
        self.system.total_bytes > 0
    }
}

#[derive(Debug, Default)]
struct ProbeCache {
    sampled_at: Option<Instant>,

    working_set_bytes: Option<u64>,

    system: SystemMemory,
}

fn cache() -> &'static Mutex<ProbeCache> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ProbeCache::default()))
}

pub fn invalidate_probe_cache() {
    if let Ok(mut guard) = cache().lock() {
        *guard = ProbeCache::default();
    }
}

pub fn sample() -> MemorySample {
    MemorySample {
        process_working_set_bytes: process_working_set().unwrap_or_default(),
        system: system_memory(),
    }
}

pub fn process_working_set() -> Option<u64> {
    if let Some(cached) = cached_working_set() {
        return Some(cached);
    }
    let measured = read_process_working_set().ok();
    if let Some(bytes) = measured {
        store_working_set(bytes);
    }
    measured
}

pub fn system_memory() -> SystemMemory {
    if let Some(cached) = cached_system_memory() {
        return cached;
    }
    let measured = read_system_memory();
    if measured.total_bytes > 0 {
        store_system_memory(measured);
    }
    measured
}

pub fn process_working_set_by_os_id(os_process_id: u32) -> Option<u64> {
    read_process_working_set_by_os_id(os_process_id)
}

pub fn current_process_working_set_bytes() -> Result<u64, String> {
    read_process_working_set().map_err(|error| error.message().to_string())
}

fn cached_working_set() -> Option<u64> {
    let guard = cache().lock().ok()?;
    let fresh = guard
        .sampled_at
        .is_some_and(|moment| moment.elapsed() < PROBE_CACHE_TTL);
    if fresh {
        guard.working_set_bytes
    } else {
        None
    }
}

fn cached_system_memory() -> Option<SystemMemory> {
    let guard = cache().lock().ok()?;
    let fresh = guard
        .sampled_at
        .is_some_and(|moment| moment.elapsed() < PROBE_CACHE_TTL);
    if fresh && guard.system.total_bytes > 0 {
        Some(guard.system)
    } else {
        None
    }
}

fn store_working_set(bytes: u64) {
    if let Ok(mut guard) = cache().lock() {
        guard.working_set_bytes = Some(bytes);
        guard.sampled_at = Some(Instant::now());
    }
}

fn store_system_memory(system: SystemMemory) {
    if let Ok(mut guard) = cache().lock() {
        guard.system = system;
        guard.sampled_at = Some(Instant::now());
    }
}

#[cfg(target_os = "windows")]
fn read_process_working_set() -> Result<u64, ProbeError> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let result = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb)
            .map_err(|error| error.to_string())
    };
    match result {
        Ok(()) => Ok(counters.WorkingSetSize as u64),
        Err(detail) => Err(ProbeError::Unavailable(detail)),
    }
}

#[cfg(target_os = "windows")]
fn read_process_working_set_by_os_id(os_process_id: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    if os_process_id == std::process::id() {
        return read_process_working_set().ok();
    }
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, os_process_id) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let measured = unsafe {
        GetProcessMemoryInfo(HANDLE(handle.0), &mut counters, counters.cb)
            .ok()
            .map(|_| counters.WorkingSetSize as u64)
    };
    let _ = unsafe { CloseHandle(handle) };
    measured
}

#[cfg(target_os = "windows")]
fn read_system_memory() -> SystemMemory {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let result = unsafe { GlobalMemoryStatusEx(&mut status) };
    if result.is_err() {
        return SystemMemory::default();
    }
    SystemMemory {
        total_bytes: status.ullTotalPhys,
        available_bytes: status.ullAvailPhys,
        load_percent: status.dwMemoryLoad,
    }
}

#[cfg(not(target_os = "windows"))]
fn read_process_working_set() -> Result<u64, ProbeError> {
    let statm = std::fs::read_to_string("/proc/self/statm")
        .map_err(|error| ProbeError::Unavailable(error.to_string()))?;
    let resident_pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| ProbeError::Unavailable("malformed resident page count".to_string()))?;
    Ok(resident_pages.saturating_mul(PAGE_BYTES))
}

#[cfg(not(target_os = "windows"))]
fn read_process_working_set_by_os_id(os_process_id: u32) -> Option<u64> {
    if os_process_id == std::process::id() {
        return read_process_working_set().ok();
    }
    let path = format!("/proc/{os_process_id}/statm");
    let statm = std::fs::read_to_string(path).ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(resident_pages.saturating_mul(PAGE_BYTES))
}

#[cfg(not(target_os = "windows"))]
fn read_system_memory() -> SystemMemory {
    let contents = match std::fs::read_to_string("/proc/meminfo") {
        Ok(contents) => contents,
        Err(_) => return SystemMemory::default(),
    };
    let mut total_bytes = 0u64;
    let mut available_bytes = 0u64;
    for line in contents.lines() {
        let mut fields = line.split_whitespace();
        let key = fields.next().unwrap_or_default();
        let value = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        match key {
            "MemTotal:" => total_bytes = value.saturating_mul(1024),
            "MemAvailable:" => available_bytes = value.saturating_mul(1024),
            _ => {}
        }
    }
    if total_bytes == 0 {
        return SystemMemory::default();
    }
    if available_bytes == 0 {
        available_bytes = total_bytes / 4;
    }
    SystemMemory {
        total_bytes,
        available_bytes,
        load_percent: (100u64
            .saturating_sub(available_bytes.saturating_mul(100) / total_bytes.max(1)))
            as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_set_is_measured_and_plausible() {
        invalidate_probe_cache();
        let bytes = current_process_working_set_bytes().expect("working set should be readable");
        assert!(bytes > 0);
        assert!(bytes < 64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn cached_sample_is_stable_within_ttl() {
        invalidate_probe_cache();
        let first = process_working_set();
        let second = process_working_set();
        assert_eq!(first, second);
    }

    #[test]
    fn system_memory_exposes_available_ratio() {
        let system = system_memory();
        if system.total_bytes > 0 {
            assert!(system.available_bytes <= system.total_bytes);
            assert!((0.0..=1.0).contains(&system.available_ratio()));
        }
    }

    #[test]
    fn current_process_id_measurements_agree() {
        let direct = current_process_working_set_bytes().ok();
        let by_id = process_working_set_by_os_id(std::process::id());
        assert!(direct.is_some() == by_id.is_some());
    }
}
