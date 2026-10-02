use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::memory_probe;
use crate::memory_tracker::{MemoryBudget, MemoryPressureLevel};
use crate::parser::parse_html;
use crate::resource_caps;
use crate::Browser;

pub const CLI_FLAG: &str = "--ram-report=";
pub const TAB_FLAG: &str = "--ram-tabs=";
pub const DEFAULT_TAB_COUNT: usize = 20;
pub const DEFAULT_TAB_HTML_BYTES: usize = 1024 * 1024;
pub const TARGET_CONVERGENCE: Duration = Duration::from_secs(3);
const MAX_RELIEF_CYCLES: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamGaugeCheck {
    pub id: String,
    pub measured: u64,
    pub budget: u64,
    pub passed: bool,
    pub informational: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RamGaugeReport {
    pub version: String,
    pub tab_count: usize,
    pub per_tab_html_bytes: usize,
    pub idle_rss_bytes: u64,
    pub core_ready_rss_bytes: u64,
    pub scenario_rss_bytes: u64,
    pub after_relief_rss_bytes: u64,
    pub system_total_bytes: u64,
    pub system_available_bytes: u64,
    pub estimate_before_bytes: usize,
    pub estimate_after_bytes: usize,
    pub aggregate_before_bytes: usize,
    pub aggregate_after_bytes: usize,
    pub soft_limit_bytes: usize,
    pub hard_limit_bytes: usize,
    pub relief_cycles: usize,
    pub pacing_millis: u64,
    pub convergence_millis: u64,
    pub checks: Vec<RamGaugeCheck>,
}

impl RamGaugeReport {
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.informational || check.passed)
    }

    pub fn check(&self, id: &str) -> Option<&RamGaugeCheck> {
        self.checks.iter().find(|check| check.id == id)
    }
}

fn gauge_html(tab: usize, target_bytes: usize) -> String {
    let sample = format!("<p id=\"t{tab}r0\">tab {tab} row 0 holds its own text</p>");
    let rows = target_bytes.div_ceil(sample.len()).max(1);
    let mut html = String::with_capacity(target_bytes + sample.len() * 2);
    html.push_str("<main><h1>Live gauge</h1>");
    for row in 0..rows {
        html.push_str(&format!(
            "<p id=\"t{tab}r{row}\">tab {tab} row {row} holds its own text</p>"
        ));
    }
    html.push_str("</main>");
    html
}

fn measured_rss_bytes() -> (u64, u64, u64) {
    memory_probe::invalidate_probe_cache();
    let sample = memory_probe::sample();
    (
        sample.process_working_set_bytes,
        sample.system.total_bytes,
        sample.system.available_bytes,
    )
}

pub fn run(tab_count: usize, per_tab_html_bytes: usize) -> RamGaugeReport {
    let tabs = tab_count.max(1);
    let (idle_rss_bytes, system_total_bytes, system_available_bytes) = measured_rss_bytes();
    let mut browser = Browser::new_in_memory();
    let (core_ready_rss_bytes, _, _) = measured_rss_bytes();

    for index in 0..tabs {
        let url = format!("https://gauge{index}.test/page");
        let html = gauge_html(index, per_tab_html_bytes);
        browser.add_tab(&url, parse_html(&html), &format!("Gauge {index}"));
        if let Some(tab) = browser.active_tab_mut() {
            tab.last_active_timestamp -= 7_200 + index as i64;
        }
    }
    browser.sync_tab_budgets();
    let (scenario_rss_bytes, _, _) = measured_rss_bytes();
    let budget = browser.memory_budget();
    let estimate_before_bytes = browser.estimate_memory().total_bytes;
    let aggregate_before_bytes = browser.resources.aggregate_bytes();

    let achievable_floor = 64 * 1024;
    let tight = MemoryBudget::from_bytes(
        ((estimate_before_bytes * 2) / tabs).max(achievable_floor),
        ((estimate_before_bytes * 3) / tabs).max(achievable_floor * 2),
    );
    let started = Instant::now();
    let mut cycles = 0usize;
    let mut pacing = Duration::ZERO;
    let mut level = MemoryPressureLevel::Critical;
    let mut previous_estimate = estimate_before_bytes;
    let mut settled = false;
    while cycles < MAX_RELIEF_CYCLES {
        let pass = browser.relieve_memory_pressure(tight, 1);
        cycles += 1;
        level = pass.level;
        if cycles == 1 {
            pacing = browser.next_pressure_interval();
        }
        let observed = browser.estimate_memory().total_bytes;
        if !level.relief_required() || observed >= previous_estimate {
            settled = true;
            break;
        }
        previous_estimate = observed;
        std::thread::sleep(browser.next_pressure_interval());
    }
    let elapsed = started.elapsed();
    let real_budget_clear = !browser.pressure_status().level.relief_required();
    let driven_by_system = browser
        .pressure_reading()
        .is_some_and(|reading| reading.driven_by_system);

    browser.sync_tab_budgets();
    let (after_relief_rss_bytes, _, _) = measured_rss_bytes();
    let estimate_after_bytes = browser.estimate_memory().total_bytes;
    let aggregate_after_bytes = browser.resources.aggregate_bytes();
    let convergence_millis = elapsed.as_millis() as u64;

    let mut checks = Vec::new();
    checks.push(RamGaugeCheck {
        id: "idle_rss_headless_core".to_string(),
        measured: core_ready_rss_bytes,
        budget: resource_caps::IDLE_UI_RSS_TARGET_BYTES as u64,
        passed: core_ready_rss_bytes <= resource_caps::IDLE_UI_RSS_TARGET_BYTES as u64,
        informational: true,
        note: "headless core without an iced window, renderer children or font atlas; \
               the windowed UI target is not measured here"
            .to_string(),
    });
    checks.push(RamGaugeCheck {
        id: "scenario_rss_inside_hard_limit".to_string(),
        measured: scenario_rss_bytes,
        budget: budget.hard_limit_bytes as u64,
        passed: scenario_rss_bytes <= budget.hard_limit_bytes as u64,
        informational: false,
        note: format!("{} tabs of ~{} bytes each", tabs, per_tab_html_bytes),
    });
    checks.push(RamGaugeCheck {
        id: "aggregate_commitment_inside_hard_limit".to_string(),
        measured: aggregate_before_bytes as u64,
        budget: budget.hard_limit_bytes as u64,
        passed: aggregate_before_bytes <= budget.hard_limit_bytes,
        informational: false,
        note: "ledger aggregate of committed and measured bytes".to_string(),
    });
    checks.push(RamGaugeCheck {
        id: "relief_reduces_estimate".to_string(),
        measured: estimate_after_bytes as u64,
        budget: estimate_before_bytes as u64,
        passed: estimate_after_bytes < estimate_before_bytes,
        informational: false,
        note: format!("cycles {cycles}"),
    });
    checks.push(RamGaugeCheck {
        id: "pressure_pacing_within_target".to_string(),
        measured: pacing.as_millis() as u64,
        budget: TARGET_CONVERGENCE.as_millis() as u64,
        passed: pacing <= TARGET_CONVERGENCE,
        informational: false,
        note: "how long the governor waits before acting again while relief is required; \
               this was a fixed 60 second poll before beta1"
            .to_string(),
    });
    checks.push(RamGaugeCheck {
        id: "relief_reaches_a_fixed_point".to_string(),
        measured: cycles as u64,
        budget: MAX_RELIEF_CYCLES as u64,
        passed: settled && cycles < MAX_RELIEF_CYCLES,
        informational: false,
        note: format!("last level {}", level.label()),
    });
    checks.push(RamGaugeCheck {
        id: "relief_execution_within_target".to_string(),
        measured: convergence_millis,
        budget: TARGET_CONVERGENCE.as_millis() as u64,
        passed: elapsed <= TARGET_CONVERGENCE,
        informational: cfg!(debug_assertions),
        note: if cfg!(debug_assertions) {
            "measured on an unoptimised build; shipping budgets are enforced in release".to_string()
        } else {
            format!("{cycles} cycles of real relief work")
        },
    });
    checks.push(RamGaugeCheck {
        id: "real_budget_satisfied_after_relief".to_string(),
        measured: estimate_after_bytes as u64,
        budget: budget.soft_limit_bytes as u64,
        passed: real_budget_clear,
        informational: driven_by_system,
        note: if driven_by_system {
            "this host is short on system memory, so pressure is no longer browser driven"
                .to_string()
        } else {
            format!("measured aggregate after relief is {aggregate_after_bytes} bytes")
        },
    });

    RamGaugeReport {
        version: crate::VERSION.to_string(),
        tab_count: tabs,
        per_tab_html_bytes,
        idle_rss_bytes,
        core_ready_rss_bytes,
        scenario_rss_bytes,
        after_relief_rss_bytes,
        system_total_bytes,
        system_available_bytes,
        estimate_before_bytes,
        estimate_after_bytes,
        aggregate_before_bytes,
        aggregate_after_bytes,
        soft_limit_bytes: budget.soft_limit_bytes,
        hard_limit_bytes: budget.hard_limit_bytes,
        relief_cycles: cycles,
        pacing_millis: pacing.as_millis() as u64,
        convergence_millis,
        checks,
    }
}

pub fn try_run_cli(args: &[String]) -> Result<Option<bool>, String> {
    let Some(argument) = args.iter().find(|argument| argument.starts_with(CLI_FLAG)) else {
        return Ok(None);
    };
    let report_path = argument
        .strip_prefix(CLI_FLAG)
        .ok_or_else(|| "ram report flag has no path".to_string())?;
    if report_path.is_empty() {
        return Err("ram report path is empty".to_string());
    }
    let tabs = args
        .iter()
        .find_map(|argument| argument.strip_prefix(TAB_FLAG))
        .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or(DEFAULT_TAB_COUNT);
    if tabs == 0 || tabs > 512 {
        return Err("ram tab count must be between 1 and 512".to_string());
    }

    let report = run(tabs, DEFAULT_TAB_HTML_BYTES);
    let output = Path::new(report_path);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let encoded = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    std::fs::write(output, encoded).map_err(|error| error.to_string())?;
    Ok(Some(report.passed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_html_reaches_its_declared_size() {
        let html = gauge_html(3, 64 * 1024);
        assert!(html.len() >= 64 * 1024);
        assert!(html.len() < 64 * 1024 * 2);
        assert!(html.starts_with("<main>"));
        assert!(html.ends_with("</main>"));
        let dom = parse_html(&html);
        assert!(crate::memory_tracker::MemoryTracker::dom_node_count(&dom) > 100);
    }

    #[test]
    fn cli_requires_a_report_path_and_a_sane_tab_count() {
        let missing = try_run_cli(&[]).expect("no ram flags is not an error");
        assert!(missing.is_none());
        assert!(try_run_cli(&[CLI_FLAG.to_string()]).is_err());
        assert!(try_run_cli(&[format!("{CLI_FLAG}out.json"), "--ram-tabs=0".to_string()]).is_err());
        assert!(
            try_run_cli(&[format!("{CLI_FLAG}out.json"), "--ram-tabs=4000".to_string()]).is_err()
        );
    }

    #[test]
    fn informational_checks_cannot_fail_the_report() {
        let mut report = RamGaugeReport {
            version: crate::VERSION.to_string(),
            tab_count: 1,
            per_tab_html_bytes: 1,
            idle_rss_bytes: 0,
            core_ready_rss_bytes: u64::MAX,
            scenario_rss_bytes: 0,
            after_relief_rss_bytes: 0,
            system_total_bytes: 0,
            system_available_bytes: 0,
            estimate_before_bytes: 0,
            estimate_after_bytes: 0,
            aggregate_before_bytes: 0,
            aggregate_after_bytes: 0,
            soft_limit_bytes: 0,
            hard_limit_bytes: 0,
            relief_cycles: 0,
            pacing_millis: 0,
            convergence_millis: 0,
            checks: vec![RamGaugeCheck {
                id: "idle_rss_headless_core".to_string(),
                measured: u64::MAX,
                budget: 1,
                passed: false,
                informational: true,
                note: String::new(),
            }],
        };
        assert!(report.passed());
        assert!(report.check("idle_rss_headless_core").is_some());
        assert!(report.check("absent").is_none());
        report.checks[0].informational = false;
        assert!(!report.passed());
    }
}
