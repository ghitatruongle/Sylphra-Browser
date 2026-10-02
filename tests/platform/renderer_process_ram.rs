use sylphra::process_architecture::ProcessRole;
use sylphra::process_coordinator::memory_limit_exceeded;
use sylphra::resource_caps;
use sylphra::sandbox::{JobObjectSandbox, SandboxPolicy};

const MB: usize = 1024 * 1024;

const RENDERER_URLS: [&str; 3] = [
    "https://ram-alpha.test/page",
    "https://ram-beta.test/page",
    "https://ram-gamma.test/page",
];

#[test]
fn the_memory_classifier_agrees_with_the_renderer_job_object_policy() {
    let limit = resource_caps::RENDERER_PROCESS_MEMORY_BYTES;
    assert_eq!(limit, 192 * MB);
    assert!(!memory_limit_exceeded(limit, (limit - 1) as u64));
    assert!(!memory_limit_exceeded(0, limit as u64));
    assert!(memory_limit_exceeded(limit, limit as u64));
    assert!(memory_limit_exceeded(limit, (limit + 1) as u64));
    assert!(memory_limit_exceeded(limit, u64::MAX));

    let role = ProcessRole::Renderer {
        origin: "https://ram-alpha.test".to_string(),
    };
    let policy = SandboxPolicy::default_for_role(&role);
    assert_eq!(policy.memory_limit_bytes, limit);
    assert!(policy.kill_on_job_close);
    assert_eq!(
        policy.allowed_origin.as_deref(),
        Some("https://ram-alpha.test")
    );
    assert!(
        policy.job_memory_limit_bytes > policy.memory_limit_bytes,
        "the OS ceiling must sit above the governance threshold or a runaway renderer is never classified"
    );
    assert_eq!(
        policy.job_memory_limit_bytes,
        resource_caps::job_memory_limit_bytes(limit)
    );

    let sandbox = JobObjectSandbox::new(700, policy);
    assert!(sandbox.check_memory_usage(limit - 1).is_ok());
    assert_eq!(
        sandbox.check_memory_usage(limit + 1).is_err(),
        memory_limit_exceeded(limit, (limit + 1) as u64)
    );
}

#[test]
fn every_child_limit_fits_the_budget_it_claims() {
    let renderer = resource_caps::RENDERER_PROCESS_MEMORY_BYTES;
    let hard = resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB);
    assert!(renderer < hard);
    assert!(renderer >= resource_caps::DOM_TRANSFER_TOTAL_BYTES);
    assert_eq!(
        ProcessRole::Media.memory_limit_bytes(),
        resource_caps::MEDIA_PROCESS_MEMORY_BYTES
    );
    assert_eq!(
        ProcessRole::Network.memory_limit_bytes(),
        resource_caps::NETWORK_PROCESS_MEMORY_BYTES
    );
    assert_eq!(
        ProcessRole::Gpu.memory_limit_bytes(),
        resource_caps::GPU_PROCESS_MEMORY_BYTES
    );
    for role in [
        ProcessRole::Renderer {
            origin: "https://ram-alpha.test".to_string(),
        },
        ProcessRole::Network,
        ProcessRole::Media,
        ProcessRole::Gpu,
    ] {
        assert!(memory_limit_exceeded(
            role.memory_limit_bytes(),
            hard as u64
        ));
    }
}

#[cfg(windows)]
mod windows_renderer_ram {
    use super::RENDERER_URLS;
    use sylphra::ipc::IpcCommand;
    use sylphra::parser::parse_html;
    use sylphra::process_architecture::ProcessRole;
    use sylphra::process_coordinator::{BrowserProcessCoordinator, ProcessLossReason};
    use sylphra::resource_caps;
    use sylphra::resource_ledger::{LedgerLimits, ResourceLedger};
    use sylphra::Browser;

    fn child_program() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_sylphra-browser-child"))
    }

    #[cfg(not(feature = "renderer-memory-harness"))]
    #[test]
    fn a_shipped_renderer_refuses_the_memory_stress_command() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut coordinator =
            BrowserProcessCoordinator::start(child_program()).expect("start coordinator");
        let process = coordinator
            .attach_tab(41, RENDERER_URLS[0])
            .expect("attach renderer");

        let reply = coordinator
            .manager
            .send_native_command(
                process,
                IpcCommand::MemoryStress {
                    bytes: (8 * 1024 * 1024) as u64,
                },
            )
            .expect("stress command round trip");
        assert!(
            !reply.accepted,
            "a released child must never honour a memory stress command"
        );
        assert!(
            reply.detail.contains("unavailable"),
            "unexpected detail: {}",
            reply.detail
        );
        assert!(
            coordinator.poll_process_memory(&ledger).is_empty(),
            "a refused stress command must not disturb the renderer"
        );
        assert_eq!(ledger.main_working_set_bytes(), 0);
        assert!(
            ledger.child_working_set_bytes() < (64 * 1024 * 1024) as u64,
            "a refused stress command must not have allocated anything, children hold {}",
            ledger.child_working_set_bytes()
        );
        assert!(coordinator.heartbeat_and_recover().is_empty());
    }

    #[cfg(feature = "renderer-memory-harness")]
    #[test]
    fn a_renderer_past_its_governance_threshold_is_memory_killed_and_recovered() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut coordinator =
            BrowserProcessCoordinator::start(child_program()).expect("start coordinator");
        let process = coordinator
            .attach_tab(42, RENDERER_URLS[0])
            .expect("attach renderer");
        let threshold = resource_caps::RENDERER_PROCESS_MEMORY_BYTES;

        assert!(coordinator.poll_process_memory(&ledger).is_empty());
        let reply = coordinator
            .manager
            .send_native_command(
                process,
                IpcCommand::MemoryStress {
                    bytes: (threshold + 16 * 1024 * 1024) as u64,
                },
            )
            .expect("stress command round trip");
        assert!(reply.accepted, "harness build must honour the command");

        let losses = coordinator.poll_process_memory(&ledger);
        assert_eq!(
            losses.len(),
            1,
            "only the stressed renderer may be reported as lost"
        );
        assert_eq!(losses[0].reason, ProcessLossReason::MemoryKilled);
        assert_eq!(losses[0].origin, "https://ram-alpha.test");
        assert_eq!(
            losses[0].tabs,
            vec![sylphra::process_architecture::TabId(42)]
        );
        assert!(
            losses[0].reported_bytes >= threshold as u64,
            "the loss must record a report past the threshold, got {}",
            losses[0].reported_bytes
        );
        assert_eq!(coordinator.native_process_count(), 4);
        assert_eq!(coordinator.recovery.recovery_count, 1);
        let replacement = coordinator
            .renderer_origins()
            .into_iter()
            .find(|(origin, _)| origin == "https://ram-alpha.test")
            .map(|(_, process)| process)
            .expect("the origin keeps a renderer after recovery");
        assert_ne!(replacement, process);
        assert_eq!(
            coordinator.tabs_for_process(replacement),
            vec![sylphra::process_architecture::TabId(42)]
        );
    }

    #[test]
    fn three_renderers_run_under_job_objects_and_report_real_ram() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut coordinator =
            BrowserProcessCoordinator::start(child_program()).expect("start coordinator");
        assert_eq!(coordinator.native_process_count(), 3);
        for (index, url) in RENDERER_URLS.iter().enumerate() {
            coordinator
                .attach_tab(index + 1, url)
                .expect("attach a renderer");
        }
        assert_eq!(coordinator.native_process_count(), 6);

        let origins = coordinator.renderer_origins();
        assert_eq!(origins.len(), 3);
        for (origin, process) in &origins {
            let child = coordinator
                .manager
                .processes
                .get(process)
                .expect("renderer metadata");
            assert!(matches!(child.metadata.role, ProcessRole::Renderer { .. }));
            assert!(child.sandbox.has_native_job());
            assert_eq!(
                child.sandbox.policy.memory_limit_bytes,
                resource_caps::RENDERER_PROCESS_MEMORY_BYTES
            );
            assert_eq!(
                child.sandbox.policy.allowed_origin.as_deref(),
                Some(origin.as_str())
            );
            assert_eq!(coordinator.tabs_for_process(*process).len(), 1);
        }

        assert!(
            coordinator.poll_process_memory(&ledger).is_empty(),
            "healthy renderers must not be reported as lost"
        );
        assert!(ledger.child_process_count() >= 3);
        assert!(ledger.child_working_set_bytes() > 0);
        assert_eq!(ledger.main_working_set_bytes(), 0);
        assert!(ledger.aggregate_bytes() > 0);
        assert!(coordinator.heartbeat_and_recover().is_empty());
    }

    #[test]
    fn a_lost_renderer_is_replaced_and_its_tabs_survive() {
        let ledger = ResourceLedger::new(LedgerLimits::default());
        let mut coordinator =
            BrowserProcessCoordinator::start(child_program()).expect("start coordinator");
        let process = coordinator
            .attach_tab(31, RENDERER_URLS[0])
            .expect("attach renderer");
        assert_eq!(coordinator.native_process_count(), 4);

        assert!(coordinator.manager.kill_native_process(process));
        let losses = coordinator.poll_process_memory(&ledger);
        assert_eq!(losses.len(), 1);
        assert_eq!(losses[0].reason, ProcessLossReason::Crashed);
        assert_eq!(
            losses[0].tabs,
            vec![sylphra::process_architecture::TabId(31)]
        );
        assert_eq!(coordinator.recovery.recovery_count, 1);
        assert_eq!(coordinator.native_process_count(), 4);

        let replacement = coordinator
            .renderer_origins()
            .into_iter()
            .find(|(origin, _)| origin == "https://ram-alpha.test")
            .map(|(_, process)| process)
            .expect("the origin keeps a renderer after recovery");
        assert_ne!(replacement, process);
        assert_eq!(
            coordinator.tabs_for_process(replacement),
            vec![sylphra::process_architecture::TabId(31)]
        );
        assert!(coordinator.heartbeat_and_recover().is_empty());
        assert!(coordinator.poll_process_memory(&ledger).is_empty());
    }

    #[test]
    fn shedding_a_renderer_discards_its_tab_and_undiscard_returns_the_url() {
        let mut browser = Browser::new_in_memory();
        assert_eq!(
            browser
                .initialize_process_architecture_from(child_program())
                .expect("initialise process architecture"),
            3
        );

        let mut urls = Vec::new();
        for url in RENDERER_URLS {
            let id = browser.add_tab(
                url,
                parse_html(&format!("<main><p>{url}</p></main>")),
                "Ram",
            );
            browser
                .attach_navigation_process(id, url)
                .expect("attach a renderer");
            urls.push(id);
        }
        assert_eq!(browser.native_renderer_count(), 6);
        browser.sync_tab_budgets();
        assert!(browser
            .committed_bytes_by_tab()
            .iter()
            .all(|(_, bytes)| *bytes > 0));
        assert!(browser.poll_renderer_memory().is_empty());

        let notes = browser.shed_renderers(1);
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("was shed") && notes[0].contains("tab(s) discarded"),
            "unexpected note: {}",
            notes[0]
        );
        assert_eq!(browser.native_renderer_count(), 5);

        let discarded: Vec<usize> = (0..urls.len())
            .filter_map(|index| {
                browser
                    .tab_by_index(index)
                    .filter(|tab| tab.is_discarded)
                    .map(|tab| tab.id)
            })
            .collect();
        assert_eq!(discarded.len(), 1);
        let lost = discarded[0];
        assert!(!browser.active_tab().unwrap().is_discarded);
        assert_eq!(browser.tab_committed_bytes(lost), 0);

        let url = browser.undiscard_tab(lost).expect("undiscard yields a url");
        assert!(RENDERER_URLS.contains(&url.as_str()));
        assert!(!browser.active_tab().unwrap().is_discarded);
        browser
            .attach_navigation_process(lost, &url)
            .expect("reattach a renderer");
        assert_eq!(browser.native_renderer_count(), 6);
        assert!(browser.poll_renderer_memory().is_empty());
    }
}
