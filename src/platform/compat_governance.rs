use std::collections::{HashMap, HashSet, VecDeque};

use crate::compat_backend::WebView2HostPolicy;
use crate::memory_probe;

pub fn webview_ceiling_bytes() -> usize {
    crate::resource_caps::mb(crate::resource_caps::SUBSYSTEM_WEBVIEW_MB)
}

pub fn webview_soft_headroom_mb() -> u32 {
    crate::resource_caps::GLOBAL_SOFT_WITH_WEBVIEW_MB
        .saturating_sub(crate::resource_caps::GLOBAL_SOFT_LIMIT_MB)
}

pub fn webview_hard_headroom_mb() -> u32 {
    crate::resource_caps::GLOBAL_HARD_WITH_WEBVIEW_MB
        .saturating_sub(crate::resource_caps::GLOBAL_HARD_LIMIT_MB)
}

pub const USER_DATA_MAX_DEPTH: usize = 8;
pub const USER_DATA_MAX_ENTRIES: usize = 20_000;
pub const USER_DATA_CACHE_DIRECTORIES: [&str; 3] = [
    "EBWebView/Default/Cache",
    "EBWebView/Default/Code Cache",
    "EBWebView/Default/GPUCache",
];

pub fn cache_roots(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    USER_DATA_CACHE_DIRECTORIES
        .iter()
        .map(|relative| root.join(relative))
        .collect()
}

pub fn collect_files(
    root: &std::path::Path,
) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    while let Some((directory, depth)) = stack.pop() {
        if depth > USER_DATA_MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= USER_DATA_MAX_ENTRIES {
                return files;
            }
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push((path, depth + 1));
            } else if metadata.is_file() {
                let modified = metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push((path, metadata.len(), modified));
            }
        }
    }
    files
}

pub fn directory_bytes(root: &std::path::Path) -> u64 {
    collect_files(root)
        .iter()
        .map(|(_, size, _)| *size)
        .fold(0u64, |total, size| total.saturating_add(size))
}

pub fn trim_cache_directories(roots: &[std::path::PathBuf], cap_bytes: u64) -> u64 {
    let mut files = Vec::new();
    let mut total = 0u64;
    for root in roots {
        for entry in collect_files(root) {
            total = total.saturating_add(entry.1);
            files.push(entry);
        }
    }
    if total <= cap_bytes {
        return 0;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut freed = 0u64;
    for (path, size, _) in files {
        if total.saturating_sub(freed) <= cap_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed = freed.saturating_add(size);
        }
    }
    freed
}

pub fn user_data_cap_bytes() -> u64 {
    crate::resource_caps::mb(crate::resource_caps::WEBVIEW_USER_DATA_MAX_MB) as u64
}

#[derive(Debug, Clone, Default)]
pub struct ProcessTree {
    children: HashMap<u32, Vec<u32>>,
}

impl ProcessTree {
    pub fn from_parent_pairs(pairs: impl IntoIterator<Item = (u32, u32)>) -> Self {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in pairs {
            if pid == parent {
                continue;
            }
            children.entry(parent).or_default().push(pid);
        }
        Self { children }
    }

    pub fn subtree(&self, root: u32) -> Vec<u32> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut ordered = Vec::new();
        visited.insert(root);
        queue.push_back(root);
        while let Some(pid) = queue.pop_front() {
            ordered.push(pid);
            if let Some(children) = self.children.get(&pid) {
                for child in children {
                    if visited.insert(*child) {
                        queue.push_back(*child);
                    }
                }
            }
        }
        ordered
    }
}

#[cfg(windows)]
pub fn snapshot_parent_pairs() -> Vec<(u32, u32)> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut pairs = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pairs;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                pairs.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    pairs
}

#[cfg(not(windows))]
pub fn snapshot_parent_pairs() -> Vec<(u32, u32)> {
    Vec::new()
}

pub fn union_subtree(tree: &ProcessTree, roots: &[u32]) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut union = Vec::new();
    for root in roots {
        for pid in tree.subtree(*root) {
            if seen.insert(pid) {
                union.push(pid);
            }
        }
    }
    union
}

pub fn union_working_set_bytes(tree: &ProcessTree, roots: &[u32]) -> u64 {
    union_subtree(tree, roots)
        .iter()
        .map(|pid| memory_probe::process_working_set_by_os_id(*pid).unwrap_or_default())
        .fold(0u64, |total, bytes| total.saturating_add(bytes))
}

pub fn trees_working_set_bytes(roots: &[u32]) -> u64 {
    if roots.is_empty() {
        return 0;
    }
    let tree = ProcessTree::from_parent_pairs(snapshot_parent_pairs());
    union_working_set_bytes(&tree, roots)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatLoadReport {
    pub live_surfaces: usize,
    pub measured_bytes: u64,
    pub ceiling_bytes: usize,
}

impl CompatLoadReport {
    pub fn from_measured(measured_bytes: u64, live_surfaces: usize) -> Self {
        Self {
            live_surfaces,
            measured_bytes,
            ceiling_bytes: webview_ceiling_bytes(),
        }
    }

    pub fn over_ceiling(self) -> bool {
        self.measured_bytes > self.ceiling_bytes as u64
    }

    pub fn should_suspend_background(self, policy: WebView2HostPolicy) -> bool {
        policy.should_suspend_background(self.live_surfaces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn subtree_walks_children_and_grandchildren() {
        let tree = ProcessTree::from_parent_pairs([(2u32, 1u32), (3, 1), (4, 2), (5, 3)]);
        let mut whole = tree.subtree(1);
        whole.sort_unstable();
        assert_eq!(whole, vec![1, 2, 3, 4, 5]);
        let mut branch = tree.subtree(2);
        branch.sort_unstable();
        assert_eq!(branch, vec![2, 4]);
    }

    #[test]
    fn subtree_terminates_on_cycles() {
        let tree = ProcessTree::from_parent_pairs([(1u32, 2u32), (2, 1)]);
        let mut got = tree.subtree(1);
        got.sort_unstable();
        assert_eq!(got, vec![1, 2]);
    }

    #[test]
    fn self_parent_pair_is_ignored() {
        let tree = ProcessTree::from_parent_pairs([(7u32, 7u32)]);
        assert_eq!(tree.subtree(7), vec![7]);
    }

    #[test]
    fn report_uses_policy_for_surplus_and_suspend() {
        let policy = WebView2HostPolicy {
            max_foreground_live: 1,
            max_concurrent: 4,
        };
        let report = CompatLoadReport {
            live_surfaces: 6,
            measured_bytes: 400_000_000,
            ceiling_bytes: 320_000_000,
        };
        assert!(report.over_ceiling());
        assert_eq!(policy.controllers_to_recycle(report.live_surfaces), 2);
        assert!(report.should_suspend_background(policy));
    }

    #[test]
    fn the_composite_headroom_matches_the_declared_limits() {
        assert_eq!(webview_soft_headroom_mb(), 300);
        assert_eq!(webview_hard_headroom_mb(), 400);
        assert!(webview_soft_headroom_mb() < webview_hard_headroom_mb());
    }

    #[test]
    fn report_within_ceiling_is_not_over() {
        let report = CompatLoadReport {
            live_surfaces: 1,
            measured_bytes: 1_000,
            ceiling_bytes: 2_000,
        };
        assert!(!report.over_ceiling());
    }

    #[test]
    fn overlapping_roots_are_counted_once() {
        let tree = ProcessTree::from_parent_pairs([(2u32, 1u32), (3, 1), (4, 2)]);
        assert_eq!(union_subtree(&tree, &[1, 2, 4]), vec![1, 2, 3, 4]);
        assert_eq!(union_subtree(&tree, &[2, 2, 2]), vec![2, 4]);
        assert!(union_subtree(&tree, &[]).is_empty());
    }

    #[test]
    fn duplicate_roots_measure_one_process_set() {
        let tree = ProcessTree::from_parent_pairs(snapshot_parent_pairs());
        let root = std::process::id();
        assert_eq!(
            union_subtree(&tree, &[root, root, root]),
            union_subtree(&tree, &[root])
        );
        assert!(union_subtree(&tree, &[root]).contains(&root));
    }

    #[test]
    fn an_absent_directory_measures_zero() {
        let missing = std::env::temp_dir().join("sylphra-compat-absent-directory");
        let _ = std::fs::remove_dir_all(&missing);
        assert_eq!(directory_bytes(&missing), 0);
        assert_eq!(trim_cache_directories(&[missing], 10), 0);
    }

    #[test]
    fn trim_removes_the_oldest_files_until_under_the_cap() {
        let root = std::env::temp_dir().join(format!("sylphra-compat-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cache = root.join("EBWebView/Default/Cache");
        if std::fs::create_dir_all(&cache).is_err() {
            return;
        }
        let older = cache.join("older.bin");
        let newer = cache.join("newer.bin");
        if std::fs::write(&older, vec![7u8; 40]).is_err()
            || std::fs::write(&newer, vec![9u8; 40]).is_err()
        {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let stamp_old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let stamp_new = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(9_000);
        assert!(set_modified(&older, stamp_old));
        assert!(set_modified(&newer, stamp_new));
        assert_eq!(directory_bytes(&cache), 80);
        assert_eq!(trim_cache_directories(&cache_roots(&root), 50), 40);
        assert_eq!(directory_bytes(&cache), 40);
        assert!(newer.exists());
        assert!(!older.exists());
        assert_eq!(trim_cache_directories(&cache_roots(&root), 50), 0);
        assert_eq!(trim_cache_directories(&cache_roots(&root), 40), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trim_never_touches_profile_state_outside_the_cache_roots() {
        let root = std::env::temp_dir().join(format!(
            "sylphra-compat-profile-guard-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let profile = root.join("EBWebView/Default");
        if std::fs::create_dir_all(&profile).is_err() {
            return;
        }
        let preferences = profile.join("Preferences");
        let cookies = profile.join("Network/Cookies");
        if std::fs::create_dir_all(cookies.parent().unwrap()).is_err()
            || std::fs::write(&preferences, vec![1u8; 400]).is_err()
            || std::fs::write(&cookies, vec![2u8; 400]).is_err()
        {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        assert_eq!(trim_cache_directories(&cache_roots(&root), 0), 0);
        assert!(preferences.exists());
        assert!(cookies.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn set_modified(path: &std::path::Path, when: std::time::SystemTime) -> bool {
        match std::fs::OpenOptions::new().append(true).open(path) {
            Ok(file) => file.set_modified(when).is_ok(),
            Err(_) => false,
        }
    }

    #[cfg(windows)]
    #[test]
    fn current_process_is_present_in_the_snapshot() {
        let pairs = snapshot_parent_pairs();
        let pid = std::process::id();
        if pairs.is_empty() {
            assert_eq!(trees_working_set_bytes(&[pid]), 0);
            return;
        }
        assert!(pairs.iter().any(|(found, _)| *found == pid));
        assert!(trees_working_set_bytes(&[pid]) > 0);
    }
}
