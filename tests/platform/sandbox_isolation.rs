use sylphra::process_architecture::{ProcessId, ProcessRole};
use sylphra::resource_caps;
use sylphra::sandbox::{JobObjectSandbox, SandboxPolicy};

#[test]
fn sandbox_per_role_limits_and_memory_caps() {
    let renderer_policy = SandboxPolicy::default_for_role(&ProcessRole::Renderer {
        origin: "https://secure.site.com".to_string(),
    });
    assert_eq!(
        renderer_policy.memory_limit_bytes,
        resource_caps::RENDERER_PROCESS_MEMORY_BYTES
    );
    assert!(
        renderer_policy.memory_limit_bytes < resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB)
    );
    assert!(renderer_policy.ui_restrictions);

    let network_policy = SandboxPolicy::default_for_role(&ProcessRole::Network);
    assert_eq!(
        network_policy.memory_limit_bytes,
        resource_caps::NETWORK_PROCESS_MEMORY_BYTES
    );

    let gpu_policy = SandboxPolicy::default_for_role(&ProcessRole::Gpu);
    assert_eq!(
        gpu_policy.memory_limit_bytes,
        resource_caps::GPU_PROCESS_MEMORY_BYTES
    );
    assert!(!gpu_policy.ui_restrictions);

    let browser_policy = SandboxPolicy::default_for_role(&ProcessRole::Browser);
    assert_eq!(
        browser_policy.memory_limit_bytes,
        resource_caps::BROWSER_PROCESS_MEMORY_BYTES
    );
    assert!(!browser_policy.kill_on_job_close);

    let mut sandbox = JobObjectSandbox::new(500, renderer_policy);
    sandbox.assign_process(ProcessId(10)).expect("assign");

    assert!(sandbox
        .check_memory_usage(resource_caps::RENDERER_PROCESS_MEMORY_BYTES / 2)
        .is_ok());
    assert!(sandbox
        .check_memory_usage(resource_caps::RENDERER_PROCESS_MEMORY_BYTES + 1)
        .is_err());
}

#[test]
fn site_origin_isolation_policy() {
    let policy = SandboxPolicy::default_for_role(&ProcessRole::Renderer {
        origin: "https://bank.example.com".to_string(),
    });

    let sandbox = JobObjectSandbox::new(501, policy);

    assert!(sandbox.validate_site_access("https://bank.example.com"));

    assert!(!sandbox.validate_site_access("https://phishing.example.org"));
}
