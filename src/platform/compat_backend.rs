use crate::resource_caps;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderBackend {
    Native,
    CompatWebview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPreference {
    #[default]
    Auto,
    ForceNative,
    ForceCompat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompatibilitySignals {
    pub renderer_fallback: bool,
    pub probe_failed: bool,
    pub blank_render: bool,
    pub unsupported_javascript: bool,
    pub dynamic_media: bool,
}

impl CompatibilitySignals {
    pub fn needs_compat_backend(&self) -> bool {
        self.renderer_fallback
            || self.probe_failed
            || self.blank_render
            || self.unsupported_javascript
            || self.dynamic_media
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BackendRouter {
    preference: BackendPreference,
    compat_available: bool,
}

impl Default for BackendRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendRouter {
    pub fn new() -> Self {
        Self {
            preference: BackendPreference::default(),
            compat_available: true,
        }
    }

    pub fn with_preference(mut self, preference: BackendPreference) -> Self {
        self.preference = preference;
        self
    }

    pub fn with_compat_available(mut self, compat_available: bool) -> Self {
        self.compat_available = compat_available;
        self
    }

    pub fn preference(&self) -> BackendPreference {
        self.preference
    }

    pub fn choose(&self, signals: &CompatibilitySignals) -> RenderBackend {
        match self.preference {
            BackendPreference::ForceNative => RenderBackend::Native,
            BackendPreference::ForceCompat if self.compat_available => RenderBackend::CompatWebview,
            BackendPreference::ForceCompat => RenderBackend::Native,
            BackendPreference::Auto if self.compat_available && signals.needs_compat_backend() => {
                RenderBackend::CompatWebview
            }
            BackendPreference::Auto => RenderBackend::Native,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WebView2HostPolicy {
    pub max_foreground_live: usize,
    pub max_concurrent: usize,
}

impl Default for WebView2HostPolicy {
    fn default() -> Self {
        Self::from_caps()
    }
}

impl WebView2HostPolicy {
    pub fn from_caps() -> Self {
        Self {
            max_foreground_live: resource_caps::WEBVIEW_MAX_LIVE_FOREGROUND,
            max_concurrent: resource_caps::WEBVIEW_MAX_CONCURRENT_CONTROLLERS,
        }
    }

    pub fn allows_new_foreground(&self, active_foreground: usize) -> bool {
        active_foreground < self.max_foreground_live
    }

    pub fn should_suspend_background(&self, live_total: usize) -> bool {
        live_total > self.max_foreground_live
    }

    pub fn controllers_to_recycle(&self, live_total: usize) -> usize {
        live_total.saturating_sub(self.max_concurrent)
    }

    pub fn foreground_demotions(&self, visible: &[usize], promoted: usize) -> Vec<usize> {
        let others: Vec<usize> = visible
            .iter()
            .copied()
            .filter(|id| *id != promoted)
            .collect();
        if self.allows_new_foreground(others.len()) {
            return Vec::new();
        }
        let keep = self.max_foreground_live.saturating_sub(1);
        others.into_iter().skip(keep).collect()
    }

    pub fn recycle_plan(&self, live_and_pending: usize, lru_order: &[usize]) -> Vec<usize> {
        let overflow = self.controllers_to_recycle(live_and_pending);
        lru_order.iter().copied().take(overflow).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals_needing_compat() -> CompatibilitySignals {
        CompatibilitySignals {
            blank_render: true,
            ..CompatibilitySignals::default()
        }
    }

    #[test]
    fn healthy_signals_stay_on_native() {
        let router = BackendRouter::new();
        let signals = CompatibilitySignals::default();
        assert_eq!(router.choose(&signals), RenderBackend::Native);
    }

    #[test]
    fn degraded_signals_switch_to_compat() {
        let router = BackendRouter::new();
        assert_eq!(
            router.choose(&signals_needing_compat()),
            RenderBackend::CompatWebview
        );
    }

    #[test]
    fn missing_runtime_keeps_native_even_when_degraded() {
        let router = BackendRouter::new().with_compat_available(false);
        assert_eq!(
            router.choose(&signals_needing_compat()),
            RenderBackend::Native
        );
    }

    #[test]
    fn force_native_overrides_degraded_signals() {
        let router = BackendRouter::new().with_preference(BackendPreference::ForceNative);
        assert_eq!(
            router.choose(&signals_needing_compat()),
            RenderBackend::Native
        );
    }

    #[test]
    fn force_compat_overrides_healthy_signals() {
        let router = BackendRouter::new().with_preference(BackendPreference::ForceCompat);
        assert_eq!(
            router.choose(&CompatibilitySignals::default()),
            RenderBackend::CompatWebview
        );
    }

    #[test]
    fn policy_uses_the_shared_ceiling_table() {
        let policy = WebView2HostPolicy::from_caps();
        assert_eq!(
            policy.max_foreground_live,
            resource_caps::WEBVIEW_MAX_LIVE_FOREGROUND
        );
        assert_eq!(
            policy.max_concurrent,
            resource_caps::WEBVIEW_MAX_CONCURRENT_CONTROLLERS
        );
    }

    #[test]
    fn policy_bounds_foreground_and_recycles_overflow() {
        let policy = WebView2HostPolicy {
            max_foreground_live: 1,
            max_concurrent: 4,
        };
        assert!(policy.allows_new_foreground(0));
        assert!(!policy.allows_new_foreground(1));
        assert!(policy.should_suspend_background(2));
        assert!(!policy.should_suspend_background(1));
        assert_eq!(policy.controllers_to_recycle(4), 0);
        assert_eq!(policy.controllers_to_recycle(6), 2);
    }

    #[test]
    fn recycle_plan_never_exceeds_the_overflow() {
        let policy = WebView2HostPolicy {
            max_foreground_live: 1,
            max_concurrent: 4,
        };
        assert!(policy.recycle_plan(4, &[1, 2, 3]).is_empty());
        assert_eq!(policy.recycle_plan(6, &[1, 2, 3]), vec![1, 2]);
        assert_eq!(policy.recycle_plan(9, &[1]), vec![1]);
    }

    #[test]
    fn promoting_a_surface_demotes_only_the_excess_foreground() {
        let policy = WebView2HostPolicy {
            max_foreground_live: 1,
            max_concurrent: 4,
        };
        assert!(policy.foreground_demotions(&[], 3).is_empty());
        assert_eq!(policy.foreground_demotions(&[1], 3), vec![1]);
        assert_eq!(policy.foreground_demotions(&[1, 2], 2), vec![1]);
        let single_foreground = WebView2HostPolicy {
            max_foreground_live: 2,
            max_concurrent: 4,
        };
        assert_eq!(single_foreground.foreground_demotions(&[1, 4], 2), vec![4]);
    }
}
