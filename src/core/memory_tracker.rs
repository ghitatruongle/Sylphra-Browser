use crate::image_loader::ImageCache;
use crate::layout::LayoutNode;
use crate::memory_probe::SystemMemory;
use crate::parser::Element;
use crate::resource_caps;
use crate::tab::Tab;

pub const BYTES_PER_DOM_NODE: usize = resource_caps::DOM_NODE_ESTIMATE_BYTES;

pub const BYTES_PER_LAYOUT_NODE: usize = resource_caps::DOM_LAYOUT_NODE_ESTIMATE_BYTES;

#[derive(Debug, Clone, Default)]
pub struct TabMemoryEstimate {
    pub dom_bytes: usize,

    pub layout_bytes: usize,

    pub history_bytes: usize,

    pub history_snapshot_bytes: usize,

    pub sleep_snapshot_bytes: usize,

    pub runtime_bytes: usize,

    pub total_bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub struct BrowserMemoryEstimate {
    pub tabs: Vec<TabMemoryEstimate>,

    pub image_cache_bytes: usize,

    pub resource_cache_bytes: usize,

    pub total_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    pub soft_limit_bytes: usize,
    pub hard_limit_bytes: usize,
}

impl MemoryBudget {
    pub fn from_mb(soft_limit_mb: u32, hard_limit_mb: u32) -> Self {
        const MB: usize = 1024 * 1024;
        Self::from_bytes(
            (soft_limit_mb as usize).saturating_mul(MB),
            (hard_limit_mb as usize).saturating_mul(MB),
        )
    }

    pub fn from_bytes(soft_limit_bytes: usize, hard_limit_bytes: usize) -> Self {
        Self {
            soft_limit_bytes: soft_limit_bytes.min(hard_limit_bytes),
            hard_limit_bytes,
        }
    }

    pub fn level_for(self, bytes: usize) -> MemoryPressureLevel {
        if self.hard_limit_bytes == 0 {
            MemoryPressureLevel::Disabled
        } else if bytes >= self.hard_limit_bytes {
            MemoryPressureLevel::Critical
        } else if bytes >= self.soft_limit_bytes {
            MemoryPressureLevel::Moderate
        } else {
            MemoryPressureLevel::Normal
        }
    }

    pub fn level_for_with_system(self, bytes: usize, system: SystemMemory) -> MemoryPressureLevel {
        let bytes_level = self.level_for(bytes);
        if bytes_level == MemoryPressureLevel::Disabled {
            return MemoryPressureLevel::Disabled;
        }
        let available = system.has_information().then_some(system.available_bytes);
        bytes_level.strongest(system_level_for(available, false))
    }

    pub fn disabled() -> Self {
        Self {
            soft_limit_bytes: 0,
            hard_limit_bytes: 0,
        }
    }

    pub fn is_enforced(self) -> bool {
        self.hard_limit_bytes > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryPressureLevel {
    Disabled,
    #[default]
    Normal,
    Moderate,
    Critical,
    Emergency,
}

impl MemoryPressureLevel {
    pub fn rank(self) -> u8 {
        match self {
            MemoryPressureLevel::Disabled => 0,
            MemoryPressureLevel::Normal => 1,
            MemoryPressureLevel::Moderate => 2,
            MemoryPressureLevel::Critical => 3,
            MemoryPressureLevel::Emergency => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MemoryPressureLevel::Disabled => "off",
            MemoryPressureLevel::Normal => "normal",
            MemoryPressureLevel::Moderate => "moderate",
            MemoryPressureLevel::Critical => "critical",
            MemoryPressureLevel::Emergency => "emergency",
        }
    }

    pub fn relief_required(self) -> bool {
        matches!(
            self,
            MemoryPressureLevel::Moderate
                | MemoryPressureLevel::Critical
                | MemoryPressureLevel::Emergency
        )
    }

    pub fn shed_immediately(self) -> bool {
        matches!(
            self,
            MemoryPressureLevel::Critical | MemoryPressureLevel::Emergency
        )
    }

    fn strongest(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

fn bytes_level_for(budget: MemoryBudget, bytes: usize, tightened: bool) -> MemoryPressureLevel {
    if budget.hard_limit_bytes == 0 {
        return MemoryPressureLevel::Disabled;
    }
    let soft = if tightened {
        budget.soft_limit_bytes * resource_caps::PRESSURE_HYSTERESIS_PERCENT / 100
    } else {
        budget.soft_limit_bytes
    };
    let hard = if tightened {
        budget.hard_limit_bytes * resource_caps::PRESSURE_HYSTERESIS_PERCENT / 100
    } else {
        budget.hard_limit_bytes
    };
    if bytes >= hard {
        MemoryPressureLevel::Critical
    } else if bytes >= soft {
        MemoryPressureLevel::Moderate
    } else {
        MemoryPressureLevel::Normal
    }
}

fn system_level_for(available_bytes: Option<u64>, tightened: bool) -> MemoryPressureLevel {
    let Some(available) = available_bytes else {
        return MemoryPressureLevel::Normal;
    };
    let scale: u64 = if tightened {
        (200 - resource_caps::PRESSURE_HYSTERESIS_PERCENT) as u64
    } else {
        100
    };
    let moderate = resource_caps::SYSTEM_MODERATE_AVAILABLE_MB * scale / 100;
    let critical = resource_caps::SYSTEM_CRITICAL_AVAILABLE_MB * scale / 100;
    let emergency = resource_caps::SYSTEM_EMERGENCY_AVAILABLE_MB * scale / 100;
    if available < emergency * 1024 * 1024 {
        MemoryPressureLevel::Emergency
    } else if available < critical * 1024 * 1024 {
        MemoryPressureLevel::Critical
    } else if available < moderate * 1024 * 1024 {
        MemoryPressureLevel::Moderate
    } else {
        MemoryPressureLevel::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressureReading {
    pub level: MemoryPressureLevel,
    pub consumed_bytes: usize,
    pub system_available_bytes: u64,
    pub driven_by_system: bool,
}

#[derive(Debug, Default)]
pub struct PressureGovernor {
    current: MemoryPressureLevel,
    last_reading: Option<PressureReading>,
}

impl PressureGovernor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn level(&self) -> MemoryPressureLevel {
        self.current
    }

    pub fn last_reading(&self) -> Option<PressureReading> {
        self.last_reading
    }

    pub fn reset(&mut self) {
        self.current = MemoryPressureLevel::default();
        self.last_reading = None;
    }

    pub fn observe(
        &mut self,
        budget: MemoryBudget,
        consumed_bytes: usize,
        system: SystemMemory,
    ) -> PressureReading {
        let available = system.has_information().then_some(system.available_bytes);
        let bytes_level = bytes_level_for(budget, consumed_bytes, false);
        let system_level = system_level_for(available, false);
        if bytes_level == MemoryPressureLevel::Disabled {
            let reading = PressureReading {
                level: MemoryPressureLevel::Disabled,
                consumed_bytes,
                system_available_bytes: available.unwrap_or_default(),
                driven_by_system: false,
            };
            self.current = MemoryPressureLevel::Disabled;
            self.last_reading = Some(reading);
            return reading;
        }
        let raw = bytes_level.strongest(system_level);
        let adopted = if raw.rank() >= self.current.rank() {
            raw
        } else {
            let strict = bytes_level_for(budget, consumed_bytes, true)
                .strongest(system_level_for(available, true));
            if strict.rank() < self.current.rank() {
                raw
            } else {
                self.current
            }
        };
        self.current = adopted;
        let reading = PressureReading {
            level: adopted,
            consumed_bytes,
            system_available_bytes: available.unwrap_or_default(),
            driven_by_system: system_level.rank() > bytes_level.rank(),
        };
        self.last_reading = Some(reading);
        reading
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryReliefReport {
    pub level: MemoryPressureLevel,
    pub before_bytes: usize,
    pub after_bytes: usize,
    pub cache_bytes_freed: usize,
    pub slept_tabs: Vec<usize>,
    pub discarded_tabs: Vec<usize>,
    pub measured_working_set_bytes: u64,
    pub system_available_bytes: u64,
    pub released_ledger_bytes: usize,
    pub relief_passes: usize,
    pub shed_renderers: usize,
}

impl MemoryReliefReport {
    pub fn bytes_freed(&self) -> usize {
        self.before_bytes.saturating_sub(self.after_bytes)
    }

    pub fn acted(&self) -> bool {
        !self.slept_tabs.is_empty()
            || !self.discarded_tabs.is_empty()
            || self.cache_bytes_freed > 0
            || self.released_ledger_bytes > 0
            || self.shed_renderers > 0
    }
}

#[derive(Debug, Default)]
pub struct MemoryTracker;

impl MemoryTracker {
    pub fn new() -> Self {
        Self
    }

    pub fn dom_node_count(dom: &Element) -> usize {
        count_dom_nodes(dom)
    }

    pub fn layout_node_count(layout: &LayoutNode) -> usize {
        count_layout_nodes(layout)
    }

    pub fn estimate_dom(dom: &Element) -> usize {
        let node_count = count_dom_nodes(dom);
        node_count.saturating_mul(BYTES_PER_DOM_NODE)
    }

    pub fn estimate_layout(layout: &LayoutNode) -> usize {
        let node_count = count_layout_nodes(layout);
        node_count.saturating_mul(BYTES_PER_LAYOUT_NODE)
    }

    pub(crate) fn estimate_history(tab: &Tab) -> usize {
        tab.history_retained_bytes()
    }

    pub fn estimate_tab(tab: &Tab) -> TabMemoryEstimate {
        let dom_bytes = tab.estimated_dom_bytes();
        let layout_bytes = tab.estimated_layout_bytes();
        let history_bytes = MemoryTracker::estimate_history(tab);

        let sleep_snapshot_bytes = tab.compressed_snapshot_bytes();
        let runtime_bytes = tab.runtime_heap_bytes();
        let total_bytes = dom_bytes
            .saturating_add(layout_bytes)
            .saturating_add(history_bytes)
            .saturating_add(sleep_snapshot_bytes)
            .saturating_add(runtime_bytes);

        TabMemoryEstimate {
            dom_bytes,
            layout_bytes,
            history_bytes,
            history_snapshot_bytes: history_bytes,
            sleep_snapshot_bytes,
            runtime_bytes,
            total_bytes,
        }
    }

    pub fn estimate_image_cache(cache: &ImageCache) -> usize {
        cache.memory_usage()
    }

    pub fn estimate_resource_cache(cache: &crate::network::ResourceCache) -> usize {
        cache.total_bytes()
    }

    pub fn estimate_browser(
        tabs: &[&Tab],
        image_cache: &ImageCache,
        resource_cache: &crate::network::ResourceCache,
    ) -> BrowserMemoryEstimate {
        let tab_estimates: Vec<TabMemoryEstimate> =
            tabs.iter().map(|t| Self::estimate_tab(t)).collect();

        let tabs_total = tab_estimates.iter().fold(0usize, |total, estimate| {
            total.saturating_add(estimate.total_bytes)
        });
        let image_cache_bytes = Self::estimate_image_cache(image_cache);
        let resource_cache_bytes = Self::estimate_resource_cache(resource_cache);
        let total_bytes = tabs_total
            .saturating_add(image_cache_bytes)
            .saturating_add(resource_cache_bytes);

        BrowserMemoryEstimate {
            tabs: tab_estimates,
            image_cache_bytes,
            resource_cache_bytes,
            total_bytes,
        }
    }

    pub fn bytes_to_mb(bytes: usize) -> f32 {
        bytes as f32 / (1024.0 * 1024.0)
    }

    pub fn format_bytes(bytes: usize) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;

        let bytes_f = bytes as f64;
        if bytes_f >= GB {
            format!("{:.1} GB", bytes_f / GB)
        } else if bytes_f >= MB {
            format!("{:.1} MB", bytes_f / MB)
        } else if bytes_f >= KB {
            format!("{:.1} KB", bytes_f / KB)
        } else {
            format!("{} B", bytes)
        }
    }
}

fn count_dom_nodes(element: &Element) -> usize {
    count_dom_nodes_with_depth(element, 0)
}

fn count_dom_nodes_with_depth(element: &Element, depth: usize) -> usize {
    if depth > resource_caps::DOM_MAX_DEPTH {
        return 1;
    }
    1 + element
        .children
        .iter()
        .map(|child| count_dom_nodes_with_depth(child, depth + 1))
        .sum::<usize>()
}

fn count_layout_nodes(node: &LayoutNode) -> usize {
    count_layout_nodes_with_depth(node, 0)
}

fn count_layout_nodes_with_depth(node: &LayoutNode, depth: usize) -> usize {
    if depth > resource_caps::DOM_MAX_DEPTH {
        return 1;
    }
    1 + node
        .children
        .iter()
        .map(|child| count_layout_nodes_with_depth(child, depth + 1))
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_probe::SystemMemory;

    #[test]
    fn test_count_dom_nodes_simple() {
        let dom = crate::parser::parse_html("<html><body><p>Hello</p></body></html>");
        let count = count_dom_nodes(&dom);

        assert!(count >= 3, "Expected at least 3 nodes, got {}", count);
    }

    #[test]
    fn test_estimate_dom_nonzero() {
        let dom = crate::parser::parse_html(
            "<html><body><div><p>Hello</p><p>World</p></div></body></html>",
        );
        let bytes = MemoryTracker::estimate_dom(&dom);
        assert!(bytes > 0);

        assert!(bytes >= 1050, "Expected >= 1050 bytes, got {}", bytes);
    }

    #[test]
    fn test_estimate_layout() {
        use crate::css_parser::parse_css;
        use crate::layout;
        let dom =
            crate::parser::parse_html("<html><body><h1>Title</h1><p>Content</p></body></html>");
        let rules = parse_css("h1 { font-size: 24px; } p { font-size: 16px; }");
        if let Some(layout_tree) = layout::create_layout_tree(&dom, &rules, 800) {
            let bytes = MemoryTracker::estimate_layout(&layout_tree);
            assert!(bytes > 0);
        }
    }

    #[test]
    fn test_estimate_tab() {
        let dom = crate::parser::parse_html("<html><body><h1>Test</h1></body></html>");
        let tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom,
            "Test".to_string(),
        );
        let estimate = MemoryTracker::estimate_tab(&tab);
        assert!(estimate.dom_bytes > 0);
        assert!(estimate.total_bytes >= estimate.dom_bytes);
    }

    #[test]
    fn test_estimate_tab_with_layout() {
        use crate::css_parser::parse_css;
        use crate::layout;
        let dom =
            crate::parser::parse_html("<html><body><h1>Test</h1><p>Content here</p></body></html>");
        let rules = parse_css("h1 { font-size: 24px; }");
        let mut tab = Tab::new(
            1,
            "https://example.com".to_string(),
            dom.clone(),
            "Test".to_string(),
        );
        if let Some(layout_tree) = layout::create_layout_tree(&dom, &rules, 800) {
            tab.layout = Some(layout_tree);
        }
        let estimate = MemoryTracker::estimate_tab(&tab);
        assert!(estimate.layout_bytes > 0);
        assert!(estimate.total_bytes > estimate.dom_bytes);
    }

    #[test]
    fn test_estimate_browser() {
        let dom1 = crate::parser::parse_html("<html><body><h1>Tab 1</h1></body></html>");
        let dom2 = crate::parser::parse_html(
            "<html><body><h1>Tab 2</h1><p>More content</p></body></html>",
        );
        let tab1 = Tab::new(1, "https://a.com".to_string(), dom1, "A".to_string());
        let tab2 = Tab::new(2, "https://b.com".to_string(), dom2, "B".to_string());
        let tabs: Vec<&Tab> = vec![&tab1, &tab2];
        let image_cache = ImageCache::new();
        let resource_cache = crate::network::ResourceCache::new();
        let estimate = MemoryTracker::estimate_browser(&tabs, &image_cache, &resource_cache);
        assert_eq!(estimate.tabs.len(), 2);
        assert!(estimate.total_bytes > 0);
    }

    #[test]
    fn test_bytes_to_mb() {
        assert_eq!(MemoryTracker::bytes_to_mb(0), 0.0);
        assert!((MemoryTracker::bytes_to_mb(1024 * 1024) - 1.0).abs() < 0.01);
        assert!((MemoryTracker::bytes_to_mb(5242880) - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(MemoryTracker::format_bytes(0), "0 B");
        assert_eq!(MemoryTracker::format_bytes(512), "512 B");
        assert!(MemoryTracker::format_bytes(1536).contains("KB"));
        assert!(MemoryTracker::format_bytes(5_242_880).contains("MB"));
        assert!(MemoryTracker::format_bytes(2_147_483_648).contains("GB"));
    }

    #[test]
    fn test_empty_dom_estimate() {
        let dom = Element::new("root");
        let bytes = MemoryTracker::estimate_dom(&dom);
        assert_eq!(bytes, BYTES_PER_DOM_NODE);
    }

    #[test]
    fn test_history_estimate() {
        let dom = crate::parser::parse_html("<html><body><h1>Test</h1></body></html>");
        let mut tab = Tab::new(1, "https://a.com".to_string(), dom.clone(), "A".to_string());

        assert_eq!(tab.history_len(), 1);

        tab.push_history(crate::tab::HistoryEntry::new(
            "https://b.com".to_string(),
            "B".to_string(),
            &dom,
        ));
        assert_eq!(tab.history_len(), 2);

        let estimate = MemoryTracker::estimate_tab(&tab);
        assert_eq!(estimate.history_bytes, tab.history_retained_bytes());
        assert!(estimate.history_bytes > 0);
    }

    #[test]
    fn history_estimate_uses_retained_snapshot_bytes() {
        let small = crate::tab::HistoryEntry::new(
            "https://example.test/small".to_string(),
            "Small".to_string(),
            &crate::parser::parse_html("<p>x</p>"),
        );
        let large = crate::tab::HistoryEntry::new(
            "https://example.test/large".to_string(),
            "Large".to_string(),
            &crate::parser::parse_html(&format!(
                "<main>{}</main>",
                "meaningful text ".repeat(20_000)
            )),
        );
        assert!(large.retained_bytes() > small.retained_bytes());
    }

    #[test]
    fn memory_budget_levels_are_deterministic() {
        let budget = MemoryBudget::from_mb(400, 500);
        assert_eq!(
            budget.level_for(399 * 1024 * 1024),
            MemoryPressureLevel::Normal
        );
        assert_eq!(
            budget.level_for(400 * 1024 * 1024),
            MemoryPressureLevel::Moderate
        );
        assert_eq!(
            budget.level_for(500 * 1024 * 1024),
            MemoryPressureLevel::Critical
        );
    }

    const MB_BYTES: usize = 1024 * 1024;

    fn unknown_system() -> SystemMemory {
        SystemMemory::default()
    }

    fn system_with_available(mb: u64) -> SystemMemory {
        SystemMemory {
            total_bytes: 16 * 1024 * mb,
            available_bytes: mb * 1024 * 1024,
            load_percent: 50,
        }
    }

    #[test]
    fn exhausted_system_memory_raises_the_level_above_our_own_usage() {
        let budget = MemoryBudget::from_mb(400, 500);
        let level = budget.level_for_with_system(10 * MB_BYTES, system_with_available(300));
        assert_eq!(level, MemoryPressureLevel::Emergency);
    }

    #[test]
    fn healthy_system_memory_keeps_low_usage_normal() {
        let budget = MemoryBudget::from_mb(400, 500);
        let level = budget.level_for_with_system(10 * MB_BYTES, system_with_available(8_000));
        assert_eq!(level, MemoryPressureLevel::Normal);
    }

    #[test]
    fn a_disabled_budget_disables_pressure_even_under_system_stress() {
        let budget = MemoryBudget::disabled();
        assert!(!budget.is_enforced());
        assert_eq!(
            budget.level_for_with_system(4_000 * MB_BYTES, system_with_available(128)),
            MemoryPressureLevel::Disabled
        );
    }

    #[test]
    fn governor_holds_the_level_until_hysteresis_releases_it() {
        let budget = MemoryBudget::from_mb(400, 500);
        let mut governor = PressureGovernor::new();
        assert_eq!(
            governor
                .observe(budget, 500 * MB_BYTES, unknown_system())
                .level,
            MemoryPressureLevel::Critical
        );
        assert_eq!(
            governor
                .observe(budget, 480 * MB_BYTES, unknown_system())
                .level,
            MemoryPressureLevel::Critical
        );
        assert_eq!(
            governor
                .observe(budget, 405 * MB_BYTES, unknown_system())
                .level,
            MemoryPressureLevel::Moderate
        );
        assert_eq!(
            governor
                .observe(budget, 100 * MB_BYTES, unknown_system())
                .level,
            MemoryPressureLevel::Normal
        );
        assert_eq!(governor.level(), MemoryPressureLevel::Normal);
    }

    #[test]
    fn governor_reacts_immediately_when_pressure_rises() {
        let budget = MemoryBudget::from_mb(400, 500);
        let mut governor = PressureGovernor::new();
        governor.observe(budget, 10 * MB_BYTES, unknown_system());
        let reading = governor.observe(budget, 200 * MB_BYTES, system_with_available(1_000));
        assert_eq!(reading.level, MemoryPressureLevel::Moderate);
        assert!(reading.driven_by_system);
    }

    #[test]
    fn system_driven_flapping_is_also_damped() {
        let budget = MemoryBudget::from_mb(400, 500);
        let mut governor = PressureGovernor::new();
        assert_eq!(
            governor
                .observe(budget, 10 * MB_BYTES, system_with_available(1_000))
                .level,
            MemoryPressureLevel::Moderate
        );
        assert_eq!(
            governor
                .observe(budget, 10 * MB_BYTES, system_with_available(1_600))
                .level,
            MemoryPressureLevel::Moderate
        );
        assert_eq!(
            governor
                .observe(budget, 10 * MB_BYTES, system_with_available(2_000))
                .level,
            MemoryPressureLevel::Normal
        );
    }

    #[test]
    fn relief_gates_match_the_level_ladder() {
        assert!(!MemoryPressureLevel::Disabled.relief_required());
        assert!(!MemoryPressureLevel::Normal.relief_required());
        assert!(MemoryPressureLevel::Moderate.relief_required());
        assert!(MemoryPressureLevel::Critical.relief_required());
        assert!(MemoryPressureLevel::Emergency.relief_required());
        assert!(MemoryPressureLevel::Emergency.shed_immediately());
        assert!(!MemoryPressureLevel::Moderate.shed_immediately());
        assert_eq!(MemoryPressureLevel::Emergency.label(), "emergency");
    }

    #[test]
    fn node_estimate_constants_come_from_the_shared_table() {
        assert_eq!(
            BYTES_PER_DOM_NODE,
            crate::resource_caps::DOM_NODE_ESTIMATE_BYTES
        );
        assert_eq!(
            BYTES_PER_LAYOUT_NODE,
            crate::resource_caps::DOM_LAYOUT_NODE_ESTIMATE_BYTES
        );
    }
}
