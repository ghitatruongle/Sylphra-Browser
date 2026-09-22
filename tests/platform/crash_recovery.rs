use sylphra::child_process::{ChildProcessManager, ProcessState};
use sylphra::crash_recovery::CrashRecoveryEngine;
use sylphra::ipc::IpcCommand;
use sylphra::process_architecture::{ProcessRole, TabId};

#[test]
fn fault_injection_kills_child_process_and_recovers_tab_state() {
    let mut cpm = ChildProcessManager::new();
    let mut recovery = CrashRecoveryEngine::new();

    let renderer_a = cpm.spawn_process(ProcessRole::Renderer {
        origin: "https://site-a.com".to_string(),
    });
    let renderer_b = cpm.spawn_process(ProcessRole::Renderer {
        origin: "https://site-b.com".to_string(),
    });

    recovery.register_tab(TabId(1), "https://site-a.com/dashboard", renderer_a);
    recovery.register_tab(TabId(2), "https://site-b.com/mail", renderer_b);

    assert_eq!(
        cpm.processes.get(&renderer_a).unwrap().state,
        ProcessState::Running
    );
    assert_eq!(
        cpm.processes.get(&renderer_b).unwrap().state,
        ProcessState::Running
    );

    assert!(recovery.inject_fault_crash(&mut cpm, renderer_a));

    assert_eq!(
        cpm.processes.get(&renderer_a).unwrap().state,
        ProcessState::Crashed
    );

    assert_eq!(
        cpm.processes.get(&renderer_b).unwrap().state,
        ProcessState::Running
    );

    assert!(cpm.send_command(renderer_a, IpcCommand::Heartbeat).is_err());

    let new_renderer_a = recovery
        .recover_process(&mut cpm, renderer_a)
        .expect("recover process");

    assert_ne!(new_renderer_a, renderer_a);
    assert_eq!(
        cpm.processes.get(&new_renderer_a).unwrap().state,
        ProcessState::Running
    );

    let tab1 = recovery.tabs.get(&TabId(1)).unwrap();
    assert_eq!(tab1.url, "https://site-a.com/dashboard");
    assert_eq!(tab1.process_id, new_renderer_a);

    let tab2 = recovery.tabs.get(&TabId(2)).unwrap();
    assert_eq!(tab2.process_id, renderer_b);

    assert_eq!(recovery.recovery_count, 1);
}

#[test]
fn watchdog_heartbeat_timeout_and_recovery() {
    let mut cpm = ChildProcessManager::new();

    let net_pid = cpm.spawn_process(ProcessRole::Network);
    assert_eq!(
        cpm.processes.get(&net_pid).unwrap().state,
        ProcessState::Running
    );

    cpm.advance_time(5001);
    let unresponsive = cpm.check_watchdog(5000);
    assert_eq!(unresponsive, vec![net_pid]);

    assert_eq!(
        cpm.processes.get(&net_pid).unwrap().state,
        ProcessState::Unresponsive
    );

    cpm.record_heartbeat(net_pid, 5001);
    assert_eq!(
        cpm.processes.get(&net_pid).unwrap().state,
        ProcessState::Running
    );
}

#[cfg(windows)]
#[test]
fn real_renderer_crash_restarts_a_new_restricted_generation() {
    let program = std::path::Path::new(env!("CARGO_BIN_EXE_sylphra-browser-child"));
    let mut manager = ChildProcessManager::new();
    let mut recovery = CrashRecoveryEngine::new();
    let old = manager
        .spawn_native_process(
            ProcessRole::Renderer {
                origin: "https://site-a.test".to_string(),
            },
            program,
        )
        .expect("spawn real renderer");
    let old_generation = manager.processes[&old].metadata.generation;
    recovery.register_tab(TabId(7), "https://site-a.test/page", old);
    assert!(recovery.inject_fault_crash(&mut manager, old));
    let replacement = recovery
        .recover_process(&mut manager, old)
        .expect("restart native renderer");
    assert!(manager.native_os_id(replacement).is_some());
    assert!(manager.processes[&replacement].metadata.generation > old_generation);
    assert_eq!(recovery.tabs[&TabId(7)].process_id, replacement);
    assert!(
        manager
            .send_native_command(replacement, sylphra::ipc::IpcCommand::Heartbeat)
            .expect("replacement IPC")
            .accepted
    );
    manager.terminate_process(replacement);
}
