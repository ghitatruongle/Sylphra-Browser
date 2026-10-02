use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessTaskInfo {
    pub tab_id: usize,
    pub title: String,
    pub url: String,
    pub memory_mb: f32,
    pub cpu_percent: f32,
    pub layout_nodes: usize,
    pub is_incognito: bool,
    pub committed_mb: f32,
    pub is_sleeping: bool,
    pub is_disk_backed: bool,
    pub is_discarded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubsystemCeilingReport {
    pub label: String,
    pub held_mb: f32,
    pub ceiling_mb: f32,
    pub utilisation_percent: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GovernanceSummary {
    pub main_rss_mb: f32,
    pub child_rss_mb: f32,
    pub native_renderer_count: usize,
    pub committed_mb: f32,
    pub reserved_mb: f32,
    pub denied_reservations: usize,
    pub denied_mb: f32,
    pub interned_savings_mb: f32,
    pub pressure_level: String,
    pub system_available_mb: f32,
    pub soft_limit_mb: f32,
    pub hard_limit_mb: f32,
    pub driven_by_system: bool,
    pub relief_required: bool,
    pub fallback_parsing: bool,
    pub subsystems: Vec<SubsystemCeilingReport>,
}

impl GovernanceSummary {
    pub fn reason(&self) -> &'static str {
        if !self.relief_required {
            return "within budget";
        }
        if self.driven_by_system {
            "system memory low"
        } else {
            "browser memory high"
        }
    }

    pub fn measured_mb(&self) -> f32 {
        self.main_rss_mb + self.child_rss_mb
    }

    pub fn parsing_path(&self) -> &'static str {
        if self.fallback_parsing {
            "fallback parser (in-process)"
        } else {
            "renderer process"
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TaskManager {
    pub open: bool,
    pub tasks: Vec<ProcessTaskInfo>,
    pub summary: GovernanceSummary,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            open: false,
            tasks: Vec::new(),
            summary: GovernanceSummary::default(),
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn update_tasks(&mut self, tasks: Vec<ProcessTaskInfo>) {
        self.tasks = tasks;
    }

    pub fn update_summary(&mut self, summary: GovernanceSummary) {
        self.summary = summary;
    }

    pub fn total_memory_mb(&self) -> f32 {
        self.tasks.iter().map(|t| t.memory_mb).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_manager() {
        let mut tm = TaskManager::new();
        assert!(!tm.open);

        tm.toggle();
        assert!(tm.open);

        tm.update_tasks(vec![
            ProcessTaskInfo {
                tab_id: 0,
                title: "Google".to_string(),
                url: "https://google.com".to_string(),
                memory_mb: 45.5,
                cpu_percent: 1.2,
                layout_nodes: 120,
                is_incognito: false,
                committed_mb: 44.0,
                is_sleeping: false,
                is_disk_backed: false,
                is_discarded: false,
            },
            ProcessTaskInfo {
                tab_id: 1,
                title: "GitHub".to_string(),
                url: "https://github.com".to_string(),
                memory_mb: 78.0,
                cpu_percent: 2.5,
                layout_nodes: 450,
                is_incognito: false,
                committed_mb: 0.0,
                is_sleeping: false,
                is_disk_backed: false,
                is_discarded: true,
            },
        ]);

        assert_eq!(tm.tasks.len(), 2);
        assert!((tm.total_memory_mb() - 123.5).abs() < 0.1);
    }

    #[test]
    fn governance_reason_follows_the_pressure_source() {
        let mut summary = GovernanceSummary {
            relief_required: true,
            driven_by_system: true,
            ..GovernanceSummary::default()
        };
        assert_eq!(summary.reason(), "system memory low");
        summary.driven_by_system = false;
        assert_eq!(summary.reason(), "browser memory high");
        summary.relief_required = false;
        assert_eq!(summary.reason(), "within budget");
        summary.main_rss_mb = 120.0;
        summary.child_rss_mb = 30.5;
        assert!((summary.measured_mb() - 150.5).abs() < 0.01);
        assert_eq!(summary.parsing_path(), "renderer process");
        summary.fallback_parsing = true;
        assert_eq!(summary.parsing_path(), "fallback parser (in-process)");
    }
}
