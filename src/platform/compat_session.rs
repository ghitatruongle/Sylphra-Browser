use std::time::Duration;

#[cfg(all(windows, feature = "compat-webview2"))]
use std::time::Instant;

use crate::compat_governance::CompatLoadReport;

#[cfg(all(windows, feature = "compat-webview2"))]
type Engine = crate::webview2_host::WebView2Host;

pub const WINDOW_TITLE_SUFFIX: &str = "Sylphra";
pub const CHROME_HEIGHT_LOGICAL: i32 = 110;
pub const MEASURE_INTERVAL: Duration = Duration::from_millis(900);

#[cfg(all(windows, feature = "compat-webview2"))]
static USER_DATA_TRIM_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(all(windows, feature = "compat-webview2"))]
#[derive(Default)]
pub struct CompatSession {
    engine: Option<Engine>,
    user_data_folder: Option<String>,
    sampled_at: Option<Instant>,
    sampled_bytes: u64,
}

#[cfg(not(all(windows, feature = "compat-webview2")))]
#[derive(Default)]
pub struct CompatSession;

impl CompatSession {
    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn new() -> Self {
        Self
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn with_user_data(user_data_folder: Option<String>) -> Self {
        Self {
            engine: None,
            user_data_folder,
            sampled_at: None,
            sampled_bytes: 0,
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn with_user_data(_user_data_folder: Option<String>) -> Self {
        Self
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    fn engine_mut(&mut self) -> &mut Engine {
        let engine = self.engine.get_or_insert_with(Engine::new);
        engine.set_user_data_folder(self.user_data_folder.clone());
        engine.set_window_probe(WINDOW_TITLE_SUFFIX, CHROME_HEIGHT_LOGICAL);
        engine
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn is_active(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.is_active())
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn is_active(&self) -> bool {
        false
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn availability(&self) -> Option<String> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.availability().map(str::to_string))
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn availability(&self) -> Option<String> {
        Some("the compatibility backend is not built into this binary".to_string())
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn request_tab(
        &mut self,
        tab_id: usize,
        url: &str,
        incognito: bool,
        config: crate::adblock::AdBlockConfig,
    ) -> String {
        let Some(surface) =
            crate::webview2_host::content_surface(WINDOW_TITLE_SUFFIX, CHROME_HEIGHT_LOGICAL)
        else {
            return format!("compat: could not find the Sylphra window handle for tab {tab_id}");
        };
        let blocker = build_blocker(config);
        let outcome = self
            .engine_mut()
            .request_surface(tab_id, &surface, url, incognito, blocker);
        match outcome {
            Ok(()) => format!(
                "compat: requesting WebView2 surface for {url} (window {:x})",
                surface.parent_hwnd
            ),
            Err(error) => format!("compat: tab {tab_id} stays native ({error})"),
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn request_tab(
        &mut self,
        _tab_id: usize,
        _url: &str,
        _incognito: bool,
        _config: crate::adblock::AdBlockConfig,
    ) -> String {
        "compat backend is not built into this binary".to_string()
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn toggle_tab(
        &mut self,
        tab_id: usize,
        url: &str,
        incognito: bool,
        config: crate::adblock::AdBlockConfig,
    ) -> String {
        if self.is_tab_active(tab_id) {
            let closed = self.close_tab(tab_id);
            format!("compat: surface closed for tab {tab_id} ({closed})")
        } else {
            self.request_tab(tab_id, url, incognito, config)
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn toggle_tab(
        &mut self,
        _tab_id: usize,
        _url: &str,
        _incognito: bool,
        _config: crate::adblock::AdBlockConfig,
    ) -> String {
        "compat backend is not built into this binary".to_string()
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn reload_tab(&mut self, tab_id: usize) -> String {
        match self
            .engine
            .as_mut()
            .map(|engine| engine.reload_surface(tab_id))
        {
            Some(Ok(())) => format!("compat: reloading tab {tab_id} in place"),
            Some(Err(error)) => format!("compat: tab {tab_id} could not reload ({error})"),
            None => format!("compat: tab {tab_id} has no surface to reload"),
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn reload_tab(&mut self, _tab_id: usize) -> String {
        "compat backend is not built into this binary".to_string()
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn navigate_tab(&mut self, tab_id: usize, url: &str) -> String {
        let outcome = self
            .engine
            .as_mut()
            .map(|engine| engine.navigate_surface(tab_id, url));
        match outcome {
            Some(Ok(())) => format!("compat: navigating tab {tab_id} to {url}"),
            Some(Err(error)) => format!("compat: tab {tab_id} could not navigate ({error})"),
            None => format!("compat: tab {tab_id} has no surface to navigate"),
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn navigate_tab(&mut self, _tab_id: usize, _url: &str) -> String {
        "compat backend is not built into this binary".to_string()
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn is_tab_active(&self, tab_id: usize) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.is_tab_active(tab_id))
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn is_tab_active(&self, _tab_id: usize) -> bool {
        false
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn is_pending(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.pending_surface_count() > 0)
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn is_pending(&self) -> bool {
        false
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn set_active_tab(&mut self, active_tab_id: Option<usize>) {
        if let Some(engine) = self.engine.as_mut() {
            engine.set_active_tab(active_tab_id);
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn set_active_tab(&mut self, _active_tab_id: Option<usize>) {}

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn refresh_bounds(&mut self) {
        use crate::webview2_host::Frame;
        let live = match self.engine.as_ref() {
            Some(engine) => engine.live_tab_ids(),
            None => return,
        };
        if live.is_empty() {
            return;
        }
        let Some(surface) =
            crate::webview2_host::content_surface(WINDOW_TITLE_SUFFIX, CHROME_HEIGHT_LOGICAL)
        else {
            return;
        };
        let frame = Frame {
            left: surface.left,
            top: surface.top,
            right: surface.right,
            bottom: surface.bottom,
        };
        if let Some(engine) = self.engine.as_mut() {
            for tab_id in live {
                let _ = engine.set_surface_bounds(tab_id, frame);
            }
        }
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn refresh_bounds(&mut self) {}

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn measure_tree_bytes(&mut self) -> u64 {
        let now = Instant::now();
        if let Some(sampled_at) = self.sampled_at {
            if now.duration_since(sampled_at) < MEASURE_INTERVAL {
                return self.sampled_bytes;
            }
        }
        let bytes = self.engine.as_ref().map_or(0, |engine| {
            crate::compat_governance::trees_working_set_bytes(&engine.browser_process_ids())
        });
        self.sampled_at = Some(now);
        self.sampled_bytes = bytes;
        bytes
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn measure_tree_bytes(&mut self) -> u64 {
        0
    }

    pub fn load_report(&mut self) -> CompatLoadReport {
        let measured = self.measure_tree_bytes();
        let live = self.live_count();
        CompatLoadReport::from_measured(measured, live)
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn tick(&mut self) -> Vec<String> {
        let notes = self
            .engine
            .as_mut()
            .map(|engine| engine.poll())
            .unwrap_or_default();
        self.release_idle_engine();
        notes
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn tick(&mut self) -> Vec<String> {
        Vec::new()
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn committed_bytes(&self) -> u64 {
        self.sampled_bytes
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn committed_bytes(&self) -> u64 {
        0
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn live_count(&self) -> usize {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.live_surface_count())
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn live_count(&self) -> usize {
        0
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn close_tab(&mut self, tab_id: usize) -> bool {
        self.sampled_at = None;
        let closed = self
            .engine
            .as_mut()
            .is_some_and(|engine| engine.close_surface(tab_id));
        self.release_idle_engine();
        closed
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn close_tab(&mut self, _tab_id: usize) -> bool {
        false
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    pub fn apply_pressure(&mut self, report: CompatLoadReport) -> Vec<String> {
        self.sampled_at = None;
        let notes = self
            .engine
            .as_mut()
            .map(|engine| engine.apply_pressure(report))
            .unwrap_or_default();
        self.release_idle_engine();
        notes
    }

    #[cfg(all(windows, feature = "compat-webview2"))]
    fn release_idle_engine(&mut self) {
        if self.engine.is_none() || self.is_active() {
            return;
        }
        self.engine = None;
        self.sampled_at = None;
        self.sampled_bytes = 0;
        trim_user_data_when_idle(self.user_data_folder.clone());
    }

    #[cfg(not(all(windows, feature = "compat-webview2")))]
    pub fn apply_pressure(&mut self, _report: CompatLoadReport) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(all(windows, feature = "compat-webview2"))]
fn trim_user_data_when_idle(folder: Option<String>) {
    use std::sync::atomic::Ordering;

    let Some(folder) = folder else {
        return;
    };
    if USER_DATA_TRIM_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("compat-user-data-trim".into())
        .spawn(move || {
            let roots = crate::compat_governance::cache_roots(std::path::Path::new(&folder));
            let cap = crate::compat_governance::user_data_cap_bytes();
            let freed = crate::compat_governance::trim_cache_directories(&roots, cap);
            if freed > 0 {
                log::info!("compat user data cache trimmed {freed} bytes over a {cap} cap");
            }
            USER_DATA_TRIM_RUNNING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        USER_DATA_TRIM_RUNNING.store(false, Ordering::Release);
    }
}

#[cfg(all(windows, feature = "compat-webview2"))]
fn resource_kind(hint: crate::webview2_host::ResourceHint) -> crate::adblock::ResourceType {
    use crate::adblock::ResourceType;
    use crate::webview2_host::ResourceHint;

    match hint {
        ResourceHint::Document => ResourceType::Document,
        ResourceHint::Script => ResourceType::Script,
        ResourceHint::Style => ResourceType::Style,
        ResourceHint::Image => ResourceType::Image,
        ResourceHint::Font => ResourceType::Font,
        ResourceHint::Media => ResourceType::Media,
        ResourceHint::Fetch => ResourceType::Fetch,
        ResourceHint::Other => ResourceType::Other,
    }
}

#[cfg(all(windows, feature = "compat-webview2"))]
fn build_blocker(config: crate::adblock::AdBlockConfig) -> crate::webview2_host::ResourceBlocker {
    use crate::adblock::AdBlocker;
    use std::sync::{Arc, Mutex};

    let engine = Arc::new(Mutex::new(AdBlocker::new(config)));
    Arc::new(
        move |resource: &str, page: &str, hint: crate::webview2_host::ResourceHint| {
            let host = crate::ui_helpers::host(page);
            let mut blocker = engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            blocker.should_block_resource(resource, host.as_deref(), resource_kind(hint))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_is_inactive_and_reports_no_surfaces() {
        let mut session = CompatSession::new();
        assert!(!session.is_active());
        assert!(!session.is_pending());
        assert_eq!(session.live_count(), 0);
        assert_eq!(session.measure_tree_bytes(), 0);
        assert_eq!(session.load_report().live_surfaces, 0);
        assert!(session.tick().is_empty());
        assert!(!session.close_tab(1));
    }

    #[test]
    fn a_request_reports_a_note_without_leaking_state() {
        let mut session = CompatSession::with_user_data(Some("unused".to_string()));
        let note = session.request_tab(1, "https://example.test", false, Default::default());
        assert!(!note.is_empty());
    }

    #[test]
    fn reloading_a_tab_without_a_surface_is_reported_not_panicked() {
        let mut session = CompatSession::new();
        assert!(!session.reload_tab(3).is_empty());
        assert!(!session.is_active());
    }

    #[test]
    fn pressure_on_an_empty_session_releases_nothing() {
        let mut session = CompatSession::new();
        let report = session.load_report();
        assert_eq!(report.live_surfaces, 0);
        assert!(session.apply_pressure(report).is_empty());
        session.refresh_bounds();
        session.set_active_tab(Some(1));
    }
}
