pub const KB: usize = 1024;
pub const MB: usize = 1024 * KB;
pub const GB: usize = 1024 * MB;

pub const fn mb(count: u32) -> usize {
    (count as usize).saturating_mul(MB)
}

pub const GLOBAL_SOFT_LIMIT_MB: u32 = 400;
pub const GLOBAL_HARD_LIMIT_MB: u32 = 500;

pub const SUBSYSTEM_DOM_TREE_MB: u32 = 192;
pub const SUBSYSTEM_LAYOUT_TREE_MB: u32 = 128;
pub const SUBSYSTEM_RUNTIME_HEAP_MB: u32 = 256;
pub const SUBSYSTEM_IMAGE_CACHE_MB: u32 = 64;
pub const SUBSYSTEM_RESOURCE_CACHE_MB: u32 = 32;
pub const SUBSYSTEM_MEDIA_MB: u32 = 320;
pub const SUBSYSTEM_GPU_MB: u32 = 96;
pub const SUBSYSTEM_WORKER_IPC_MB: u32 = 64;
pub const SUBSYSTEM_STORAGE_WRITE_MB: u32 = 32;

pub const TAB_LIVE_BUDGET_BYTES: usize = mb(64);
pub const TAB_RUNTIME_HEAP_BYTES: usize = mb(32);
pub const TAB_HISTORY_ENTRY_CAP: usize = 60;
pub const TAB_RAM_SNAPSHOT_CAP: usize = 2;
pub const TAB_HISTORY_SNAPSHOT_BYTES: usize = mb(16);
pub const TAB_DISK_SNAPSHOT_BYTES: usize = mb(128);

pub const DOM_NODE_ESTIMATE_BYTES: usize = 210;
pub const DOM_LAYOUT_NODE_ESTIMATE_BYTES: usize = 320;
pub const DOM_MAX_DEPTH: usize = 1_000;
pub const DOM_MAX_NODES: usize = 200_000;
pub const DOM_TRANSFER_TOTAL_BYTES: usize = mb(32);
pub const DOM_TRANSFER_SKELETON_DEPTH: usize = 64;

pub const INTERNED_STRINGS_MAX_ENTRIES: usize = 16_384;
pub const INTERNED_STRINGS_MAX_BYTES: usize = mb(4);

pub const SNAPSHOT_JSON_RAM_BYTES: usize = mb(8);
pub const SNAPSHOT_DISK_QUEUE_BYTES: usize = mb(128);

pub const CSS_MAX_SOURCE_BYTES: usize = mb(4);
pub const CSS_MAX_RULES: usize = 20_000;

pub const JS_MAX_SOURCE_BYTES: usize = 2 * MB;
pub const JS_MAX_STRING_BYTES: usize = 2 * MB;
pub const JS_MAX_STRING_BUDGET_BYTES: usize = mb(32);
pub const JS_MAX_EVAL_STEPS: u64 = 5_000_000;
pub const JS_MAX_TOKENS: usize = 262_144;

pub const IMAGE_DOWNLOAD_BYTES: u64 = 50 * MB as u64;
pub const IMAGE_DECODED_BYTES: usize = mb(64);
pub const IMAGE_MAX_EDGE_PX: u32 = 4_096;
pub const IMAGE_MAX_PIXELS: u64 = 4_096 * 4_096;
pub const IMAGE_BYTES_PER_PIXEL: u64 = 4;
pub const IMAGE_CACHE_DEFAULT_MB: u32 = 24;
pub const IMAGE_CACHE_MIN_MB: u32 = 8;
pub const IMAGE_CACHE_MAX_MB: u32 = 128;

pub const RESOURCE_CACHE_BYTES: usize = mb(32);
pub const RESOURCE_CACHE_DEFAULT_BYTES: usize = mb(64);

pub const PAGE_BODY_BYTES: u64 = 50 * MB as u64;
pub const DOWNLOAD_BODY_BYTES: u64 = 100 * MB as u64;
pub const SCHEDULER_RESPONSE_BYTES: usize = mb(50);
pub const DECOMPRESSION_MAX_RATIO: usize = 100;

pub const GPU_SOURCE_AGGREGATE_BYTES: usize = mb(96);
pub const WORKER_PROCESS_MEMORY_BYTES: usize = mb(192);
pub const WORKER_REQUEST_BYTES: usize = mb(64);
pub const WORKER_RESPONSE_BYTES: usize = mb(32);
pub const RENDERER_PROCESS_MEMORY_BYTES: usize = mb(192);
pub const NETWORK_PROCESS_MEMORY_BYTES: usize = mb(192);
pub const MEDIA_PROCESS_MEMORY_BYTES: usize = mb(320);
pub const GPU_PROCESS_MEMORY_BYTES: usize = mb(192);
pub const BROWSER_PROCESS_MEMORY_BYTES: usize = mb(2_048);

pub const IPC_MESSAGE_BYTES: usize = 4 * MB;
pub const IPC_CHUNK_BYTES: usize = 512 * KB;
pub const RENDERER_KEEP_MIN_PROCESSES: usize = 1;

pub const JOB_MEMORY_HEADROOM_PERCENT: usize = 125;

pub const fn job_memory_limit_bytes(governance_threshold_bytes: usize) -> usize {
    governance_threshold_bytes.saturating_mul(JOB_MEMORY_HEADROOM_PERCENT) / 100
}

pub const MEDIA_DECODED_BYTES: usize = mb(64);
pub const MEDIA_MSE_QUEUED_BYTES: usize = mb(256);

pub const SYSTEM_MODERATE_AVAILABLE_MB: u64 = 1_536;
pub const SYSTEM_CRITICAL_AVAILABLE_MB: u64 = 768;
pub const SYSTEM_EMERGENCY_AVAILABLE_MB: u64 = 384;

pub const PRESSURE_HYSTERESIS_PERCENT: usize = 90;
pub const PRESSURE_PULSE: std::time::Duration = std::time::Duration::from_millis(250);
pub const PRESSURE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(5);
pub const PRESSURE_IDLE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);

pub const RELIEF_GROWTH_TRIGGER_BYTES: usize = mb(8);
pub const RELIEF_PASS_TAB_PERCENT: usize = 25;
pub const RELIEF_KEEP_MIN_TABS: usize = 2;
pub const RELIEF_STALE_CACHE_SECONDS: u64 = 60;

pub const IDLE_UI_RSS_TARGET_BYTES: usize = mb(90);

#[cfg(test)]
#[allow(clippy::assertions_on_constants)]
mod tests {
    use super::*;

    #[test]
    fn global_limits_are_ordered() {
        assert!(GLOBAL_SOFT_LIMIT_MB < GLOBAL_HARD_LIMIT_MB);
        assert_eq!(mb(1), MB);
    }

    #[test]
    fn no_single_subsystem_ceiling_exceeds_the_hard_limit() {
        let hard = mb(GLOBAL_HARD_LIMIT_MB);
        let ceilings = [
            SUBSYSTEM_DOM_TREE_MB,
            SUBSYSTEM_LAYOUT_TREE_MB,
            SUBSYSTEM_RUNTIME_HEAP_MB,
            SUBSYSTEM_IMAGE_CACHE_MB,
            SUBSYSTEM_RESOURCE_CACHE_MB,
            SUBSYSTEM_MEDIA_MB,
            SUBSYSTEM_GPU_MB,
            SUBSYSTEM_WORKER_IPC_MB,
            SUBSYSTEM_STORAGE_WRITE_MB,
        ];
        for ceiling in ceilings {
            assert!(
                mb(ceiling) <= hard,
                "subsystem ceiling {ceiling} MB too large"
            );
        }
    }

    #[test]
    fn media_caps_honour_the_declared_budget() {
        assert_eq!(MEDIA_DECODED_BYTES, mb(64));
        assert_eq!(MEDIA_MSE_QUEUED_BYTES, mb(256));
        assert!(MEDIA_MSE_QUEUED_BYTES > MEDIA_DECODED_BYTES);
    }

    #[test]
    fn transfer_caps_fit_inside_ipc_frames() {
        assert!(IPC_MESSAGE_BYTES < DOM_TRANSFER_TOTAL_BYTES);
        assert!(WORKER_RESPONSE_BYTES > IPC_MESSAGE_BYTES);
    }

    #[test]
    fn system_pressure_thresholds_are_ordered() {
        assert!(SYSTEM_MODERATE_AVAILABLE_MB > SYSTEM_CRITICAL_AVAILABLE_MB);
        assert!(SYSTEM_CRITICAL_AVAILABLE_MB > SYSTEM_EMERGENCY_AVAILABLE_MB);
    }

    #[test]
    fn child_process_limits_stay_under_the_global_hard_limit() {
        let hard = mb(GLOBAL_HARD_LIMIT_MB);
        for limit in [
            RENDERER_PROCESS_MEMORY_BYTES,
            NETWORK_PROCESS_MEMORY_BYTES,
            MEDIA_PROCESS_MEMORY_BYTES,
            GPU_PROCESS_MEMORY_BYTES,
            WORKER_PROCESS_MEMORY_BYTES,
        ] {
            assert!(
                limit < hard,
                "child limit {limit} exceeds hard limit {hard}"
            );
        }
        assert!(RENDERER_PROCESS_MEMORY_BYTES >= DOM_TRANSFER_TOTAL_BYTES);
        assert!(WORKER_PROCESS_MEMORY_BYTES > WORKER_RESPONSE_BYTES);
    }

    #[test]
    fn the_interning_pool_is_bounded() {
        assert!(INTERNED_STRINGS_MAX_ENTRIES > 0);
        assert!(INTERNED_STRINGS_MAX_BYTES < mb(SUBSYSTEM_DOM_TREE_MB));
    }

    #[test]
    fn the_job_object_ceiling_sits_above_the_governance_threshold() {
        for threshold in [
            RENDERER_PROCESS_MEMORY_BYTES,
            NETWORK_PROCESS_MEMORY_BYTES,
            MEDIA_PROCESS_MEMORY_BYTES,
            GPU_PROCESS_MEMORY_BYTES,
        ] {
            let ceiling = job_memory_limit_bytes(threshold);
            assert!(
                ceiling > threshold,
                "threshold {threshold} must be crossed before the OS denies it"
            );
            assert!(ceiling <= threshold * 2);
        }
        assert_eq!(job_memory_limit_bytes(mb(192)), mb(240));
        assert_eq!(job_memory_limit_bytes(0), 0);
    }
}
