use sylphra::memory_probe::SystemMemory;
use sylphra::memory_tracker::{MemoryBudget, MemoryPressureLevel, PressureGovernor};
use sylphra::parser::parse_html;
use sylphra::resource_caps;
use sylphra::resource_ledger::{DenyScope, LedgerLimits, OwnerId, ResourceLedger, SubsystemId};
use sylphra::string_pool::interned_savings_bytes;
use sylphra::tab::WakeResult;
use sylphra::Browser;

const MB: usize = 1024 * 1024;
const GB: u64 = 1_024 * MB as u64;

fn plentiful_system() -> SystemMemory {
    SystemMemory {
        total_bytes: 16 * GB,
        available_bytes: 9 * GB,
        load_percent: 44,
    }
}

#[test]
fn one_thousand_ledger_cycles_leave_nothing_held() {
    let ledger = ResourceLedger::new(LedgerLimits::default());
    for cycle in 0..1_000 {
        let owner = OwnerId::Tab(cycle % 20);
        {
            let mut reservation = ledger
                .reserve(SubsystemId::DomTree, owner, 4_096)
                .expect("inside the dom ceiling");
            if cycle % 3 == 0 {
                reservation.commit();
            }
            if cycle % 5 == 0 {
                reservation.shrink(1_024);
            }
        }
        let lease = ledger
            .lease(SubsystemId::LayoutTree, owner, 2_048)
            .expect("inside the layout ceiling");
        drop(lease);
        ledger.release_owner(owner);
    }
    let totals = ledger.totals();
    assert_eq!(totals.committed_bytes, 0);
    assert_eq!(totals.reserved_bytes, 0);
    assert_eq!(totals.denials, 0);
    assert_eq!(ledger.snapshot().live_reservations, 0);
    assert_eq!(ledger.grant_count(), 2_000);
}

#[test]
fn dropping_a_reservation_returns_every_uncommitted_byte() {
    let ledger = ResourceLedger::new(LedgerLimits::default());
    for bytes in [1_usize, 1_024, 65_536, 3_000_000] {
        let committed = {
            let mut reservation = ledger
                .reserve(SubsystemId::RuntimeHeap, OwnerId::Global, bytes)
                .expect("reserve");
            assert_eq!(reservation.held_bytes(), bytes);
            reservation.commit_bytes(bytes / 2);
            assert_eq!(ledger.totals().committed_bytes, bytes / 2);
            reservation.shrink(bytes / 4);
            bytes / 2
        };
        let totals = ledger.totals();
        assert_eq!(totals.reserved_bytes, 0);
        assert_eq!(totals.committed_bytes, committed);
        assert_eq!(ledger.release_owner(OwnerId::Global), committed);
        assert_eq!(ledger.totals().committed_bytes, 0);
        assert_eq!(ledger.snapshot().live_reservations, 0);
    }
}

#[test]
fn every_subsystem_ceiling_fits_inside_the_global_hard_limit() {
    let ledger = ResourceLedger::new(LedgerLimits::default());
    let hard = ledger.limits().hard_bytes;
    assert_eq!(hard, resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB));
    for subsystem in SubsystemId::all() {
        let ceiling = ledger.ceiling_bytes(subsystem);
        assert!(ledger.is_registered(subsystem));
        assert!(ceiling > 0, "{subsystem} has no ceiling");
        assert!(ceiling <= hard, "{subsystem} ceiling {ceiling} > {hard}");
        assert_eq!(ceiling, subsystem.default_ceiling_bytes());
    }
}

#[test]
fn an_oversized_request_is_refused_before_anything_is_held() {
    let ledger = ResourceLedger::new(LedgerLimits::default());
    let held = ledger
        .lease(SubsystemId::Gpu, OwnerId::Global, 90 * MB)
        .expect("inside the gpu aggregate");
    assert_eq!(ledger.totals().committed_bytes, 90 * MB);
    let denied = ledger
        .lease(SubsystemId::Gpu, OwnerId::Global, 20 * MB)
        .expect_err("past the gpu aggregate");
    assert_eq!(denied.scope, DenyScope::SubsystemCeiling);
    assert_eq!(denied.subsystem, SubsystemId::Gpu);
    assert_eq!(denied.available_bytes, 6 * MB);
    assert!(denied.message().contains("denied"));
    assert_eq!(ledger.totals().committed_bytes, 90 * MB);
    assert_eq!(ledger.totals().denials, 1);
    drop(held);
    assert_eq!(ledger.totals().committed_bytes, 0);
}

#[test]
fn the_global_hard_limit_binds_the_sum_of_subsystems() {
    let ledger = ResourceLedger::new(LedgerLimits::byte_limits(10 * MB, 20 * MB));
    ledger.register_subsystem(SubsystemId::DomTree, 100 * MB);
    ledger.register_subsystem(SubsystemId::Media, 100 * MB);
    let mut held = Vec::new();
    let mut scopes = Vec::new();
    for index in 0..8 {
        let subsystem = if index % 2 == 0 {
            SubsystemId::DomTree
        } else {
            SubsystemId::Media
        };
        match ledger.lease(subsystem, OwnerId::Global, 4 * MB) {
            Ok(lease) => held.push(lease),
            Err(denied) => scopes.push(denied.scope),
        }
    }
    assert_eq!(held.len(), 5);
    assert_eq!(scopes, vec![DenyScope::GlobalHardLimit; 3]);
    assert_eq!(ledger.totals().committed_bytes, 20 * MB);
    assert_eq!(ledger.totals().headroom_bytes, 0);
}

#[test]
fn pressure_hysteresis_does_not_oscillate_at_the_boundary() {
    let budget = MemoryBudget::from_bytes(400 * MB, 500 * MB);
    let mut governor = PressureGovernor::new();
    let cases = [
        (300 * MB, MemoryPressureLevel::Normal),
        (420 * MB, MemoryPressureLevel::Moderate),
        (520 * MB, MemoryPressureLevel::Critical),
        (460 * MB, MemoryPressureLevel::Critical),
        (445 * MB, MemoryPressureLevel::Moderate),
        (395 * MB, MemoryPressureLevel::Moderate),
        (350 * MB, MemoryPressureLevel::Normal),
    ];
    for (consumed, expected) in cases {
        let reading = governor.observe(budget, consumed, plentiful_system());
        assert_eq!(
            reading.level,
            expected,
            "consumed {consumed} settled on {}",
            reading.level.label()
        );
        assert!(!reading.driven_by_system);
    }
    assert_eq!(governor.level(), MemoryPressureLevel::Normal);
}

#[test]
fn scarce_system_memory_escalates_pressure_without_browser_growth() {
    let budget = MemoryBudget::from_bytes(400 * MB, 500 * MB);
    let mut governor = PressureGovernor::new();
    let scarce = SystemMemory {
        total_bytes: 8 * GB,
        available_bytes: 300 * MB as u64,
        load_percent: 96,
    };
    let reading = governor.observe(budget, 60 * MB, scarce);
    assert_eq!(reading.level, MemoryPressureLevel::Emergency);
    assert!(reading.driven_by_system);
    assert!(reading.level.shed_immediately());

    let tightening = SystemMemory {
        total_bytes: 8 * GB,
        available_bytes: 1_200 * MB as u64,
        load_percent: 85,
    };
    let reading = governor.observe(budget, 60 * MB, tightening);
    assert_eq!(reading.level, MemoryPressureLevel::Moderate);

    let reading = governor.observe(budget, 60 * MB, plentiful_system());
    assert_eq!(reading.level, MemoryPressureLevel::Normal);
    assert!(!reading.level.relief_required());
}

#[test]
fn the_relief_loop_reads_measured_ram_and_paces_itself() {
    let mut browser = Browser::new_in_memory();
    let html = format!(
        "<main><h1>Budget</h1>{}</main>",
        "<p>every row is retained until relief</p>".repeat(2_000)
    );
    for index in 0..6 {
        browser.add_tab(
            &format!("https://paced.test/{index}"),
            parse_html(&html),
            &format!("Paced {index}"),
        );
    }
    let estimate = browser.estimate_memory().total_bytes;
    assert!(estimate > 0);
    assert_eq!(
        browser.next_pressure_interval(),
        resource_caps::PRESSURE_HEARTBEAT
    );

    let tight = MemoryBudget::from_bytes(estimate / 4, estimate / 3);
    let report = browser.relieve_memory_pressure(tight, 2);
    assert!(report.level.relief_required());
    assert!(report.level.rank() >= MemoryPressureLevel::Critical.rank());
    assert!(report.measured_working_set_bytes > 0);
    assert!(report.system_available_bytes > 0);
    assert!(report.acted());
    assert!(report.relief_passes > 0);
    assert_eq!(
        browser.next_pressure_interval(),
        resource_caps::PRESSURE_PULSE
    );

    let after = browser.relieve_memory_pressure(tight, 2);
    assert!(after.after_bytes <= report.after_bytes);
}

#[test]
fn tab_budgets_reconcile_to_held_ram_and_shrink_on_release() {
    let mut browser = Browser::new_in_memory();
    let html = format!(
        "<main>{}</main>",
        "<section><p>budgeted content</p></section>".repeat(4_000)
    );
    let first = browser.add_tab("https://budget.test/a", parse_html(&html), "First");
    let second = browser.add_tab("https://budget.test/b", parse_html(&html), "Second");
    assert_eq!(browser.committed_bytes_by_tab().len(), 2);

    browser.sync_tab_budgets();
    let per_tab = browser.tab_committed_bytes(second);
    assert!(per_tab > 0, "a live tab must hold a ledger commitment");
    let totals = browser.resources.totals();
    let held_by_tabs: usize = browser
        .committed_bytes_by_tab()
        .iter()
        .map(|(_, bytes)| *bytes)
        .sum();
    assert_eq!(held_by_tabs, browser.tab_committed_bytes(first) + per_tab);
    assert!(
        totals.committed_bytes >= held_by_tabs,
        "committed {} but tabs hold {}",
        totals.committed_bytes,
        held_by_tabs
    );
    assert_eq!(totals.reserved_bytes, 0);

    browser.sync_tab_budgets();
    assert_eq!(browser.tab_committed_bytes(second), per_tab);

    browser.active_tab_mut().unwrap().discard();
    browser.resources.release_owner(OwnerId::Tab(second));
    assert_eq!(browser.tab_committed_bytes(second), 0);
    assert!(browser.tab_committed_bytes(first) > 0);
}

#[test]
fn a_disk_backed_tab_keeps_its_restore_contract() {
    let mut browser = Browser::new_in_memory();
    let id = browser.add_tab(
        "https://disk.test/page",
        parse_html("<main><h1>Written to disk</h1><p>The RAM copy is gone.</p></main>"),
        "Disk",
    );
    let directory =
        std::env::temp_dir().join(format!("sylphra-snapshots-{}-{}", std::process::id(), id));
    browser.sync_tab_budgets();
    let committed_before = browser.tab_committed_bytes(id);
    let released = browser
        .active_tab_mut()
        .unwrap()
        .spill_to_disk(&directory)
        .expect("spill to disk");
    assert!(released > 0);
    assert!(browser.active_tab().unwrap().is_disk_backed());
    browser.sync_tab_budgets();
    assert!(browser.tab_committed_bytes(id) < committed_before);
    assert_eq!(
        browser.wake_tab(id),
        WakeResult::RestoredFromCache,
        "a disk-backed tab must wake from its snapshot"
    );
    assert!(browser.render_current().contains("Written to disk"));
    assert!(!browser.active_tab().unwrap().is_disk_backed());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn repeated_tag_and_attribute_names_share_one_allocation() {
    let before = interned_savings_bytes();
    let dom = parse_html(&format!(
        "<section>{}</section>",
        "<p class=\"row\">cell</p>".repeat(600)
    ));
    assert_eq!(dom.tag, "section");
    let first = dom.children.first().expect("a paragraph");
    let second = dom.children.get(1).expect("a second paragraph");
    assert_eq!(first.tag, "p");
    assert_eq!(second.tag, "p");
    assert!(std::sync::Arc::ptr_eq(
        first.tag.as_arc(),
        second.tag.as_arc()
    ));

    let first_name = first.attrs.keys().next().expect("a class attribute");
    let second_name = second.attrs.keys().next().expect("a class attribute");
    assert_eq!(first_name, "class");
    assert!(std::sync::Arc::ptr_eq(
        first_name.as_arc(),
        second_name.as_arc()
    ));

    let saved_here = interned_savings_bytes() - before;
    assert!(
        saved_here >= 599 * (1 + 5),
        "600 repeated names must record their deduplicated bytes, saved {saved_here}"
    );
}

#[test]
fn measured_ram_is_reported_to_the_task_manager() {
    let mut browser = Browser::new_in_memory();
    browser.add_tab(
        "https://measured.test/page",
        parse_html("<main><p>measured</p></main>"),
        "Measured",
    );
    let reading = browser.pressure_status();
    assert!(reading.consumed_bytes > 0);
    let measured = browser.measured_working_set_bytes();
    assert!(
        measured > MB as u64,
        "the probe must report a real working set, got {measured}"
    );
    assert_eq!(browser.measured_main_working_set_bytes(), measured);
    assert_eq!(browser.measured_child_working_set_bytes(), 0);
    assert_eq!(browser.native_renderer_count(), 0);

    let denied = browser
        .resources
        .lease(SubsystemId::Media, OwnerId::Global, 4 * GB as usize);
    assert!(denied.is_err());
    assert_eq!(browser.resource_denials(), 1);

    let summary = browser.governance_summary();
    assert!(summary.main_rss_mb > 1.0);
    assert_eq!(summary.child_rss_mb, 0.0);
    assert_eq!(summary.denied_reservations, 1);
    assert!(summary.denied_mb > 1_000.0);
    assert_eq!(summary.parsing_path(), "renderer process");
    assert_eq!(summary.reason(), "within budget");
    assert!(summary.measured_mb() > 0.0);
    assert!(summary.soft_limit_mb <= summary.hard_limit_mb);
}
