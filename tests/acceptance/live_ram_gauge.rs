use sylphra::ram_gauge::{RamGaugeReport, CLI_FLAG, TAB_FLAG, TARGET_CONVERGENCE};
use sylphra::resource_caps;

const MB: u64 = 1024 * 1024;

fn report_path() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "sylphra-ram-gauge-{}-{stamp}.json",
        std::process::id()
    ))
}

fn measure(tabs: usize) -> (RamGaugeReport, std::path::PathBuf) {
    let output = report_path();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_sylphra"))
        .arg(format!("{CLI_FLAG}{}", output.display()))
        .arg(format!("{TAB_FLAG}{tabs}"))
        .status()
        .expect("the shipped binary must accept the ram gauge flag");
    let encoded = std::fs::read(&output).expect("a ram gauge report must be written");
    let report: RamGaugeReport =
        serde_json::from_slice(&encoded).expect("the ram gauge report must be valid json");
    assert_eq!(
        status.code().unwrap_or_default(),
        if report.passed() { 0 } else { 2 },
        "the exit code must follow the report: {report:?}"
    );
    (report, output)
}

#[test]
fn the_shipped_binary_reports_measured_ram_for_a_twenty_tab_session() {
    let (report, output) = measure(20);
    let render = || format!("{report:#?}");

    assert_eq!(report.version, sylphra::VERSION, "{}", render());
    assert_eq!(report.tab_count, 20);
    assert_eq!(report.per_tab_html_bytes, 1024 * 1024);
    assert!(report.idle_rss_bytes > MB, "{}", render());
    assert!(report.core_ready_rss_bytes > report.idle_rss_bytes);
    assert!(report.scenario_rss_bytes > report.core_ready_rss_bytes);
    assert!(report.system_total_bytes > MB * 512);
    assert!(report.system_available_bytes > 0);
    assert_eq!(
        report.hard_limit_bytes as u64,
        resource_caps::mb(resource_caps::GLOBAL_HARD_LIMIT_MB) as u64
    );
    assert!(report.soft_limit_bytes < report.hard_limit_bytes);

    let aggregate = report
        .check("aggregate_commitment_inside_hard_limit")
        .expect("aggregate check must be reported");
    assert!(aggregate.passed, "{}", render());
    let scenario = report
        .check("scenario_rss_inside_hard_limit")
        .expect("scenario check must be reported");
    assert!(scenario.passed, "{}", render());
    let relief = report
        .check("relief_reduces_estimate")
        .expect("relief check must be reported");
    assert!(relief.passed, "{}", render());
    let pacing = report
        .check("pressure_pacing_within_target")
        .expect("pacing check must be reported");
    assert!(pacing.passed, "{}", render());
    assert_eq!(
        pacing.measured,
        resource_caps::PRESSURE_PULSE.as_millis() as u64,
        "{}",
        render()
    );
    assert!(
        pacing.measured <= TARGET_CONVERGENCE.as_millis() as u64,
        "{}",
        render()
    );
    let fixed_point = report
        .check("relief_reaches_a_fixed_point")
        .expect("fixed point check must be reported");
    assert!(fixed_point.passed, "{}", render());
    let real_budget = report
        .check("real_budget_satisfied_after_relief")
        .expect("real budget check must be reported");
    assert!(
        real_budget.passed || real_budget.informational,
        "{}",
        render()
    );
    assert!(report.estimate_after_bytes < report.estimate_before_bytes);
    assert!(report.relief_cycles > 0, "{}", render());
    assert!(report.after_relief_rss_bytes > MB);
    assert!(report.passed(), "{}", render());

    println!(
        "ram gauge: idle_core={}MB scenario={}MB after_relief={}MB estimate_before={}MB \
         estimate_after={}MB cycles={} pacing_ms={} relief_ms={} system_free={}MB",
        report.core_ready_rss_bytes / MB,
        report.scenario_rss_bytes / MB,
        report.after_relief_rss_bytes / MB,
        report.estimate_before_bytes as u64 / MB,
        report.estimate_after_bytes as u64 / MB,
        report.relief_cycles,
        report.pacing_millis,
        report.convergence_millis,
        report.system_available_bytes / MB
    );
    let _ = std::fs::remove_file(output);
}

#[test]
fn a_small_session_still_pays_for_the_relief_loop() {
    let (report, output) = measure(3);
    assert!(report.passed(), "{report:#?}");
    assert_eq!(report.tab_count, 3);
    assert!(report.estimate_before_bytes > 0);
    assert!(report.pacing_millis > 0);
    let _ = std::fs::remove_file(output);
}

#[test]
fn an_unusable_tab_count_is_refused_without_writing_a_report() {
    let output = report_path();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_sylphra"))
        .arg(format!("{CLI_FLAG}{}", output.display()))
        .arg(format!("{TAB_FLAG}4000"))
        .status()
        .expect("the shipped binary must accept the ram gauge flag");
    assert_ne!(status.code().unwrap_or_default(), 0);
    assert!(!output.exists());
}
