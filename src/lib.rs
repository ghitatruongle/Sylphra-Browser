pub const VERSION: &str = env!("CARGO_PKG_VERSION");

extern crate self as ghitabrowser;
extern crate self as sylphra;

pub mod core;
pub mod engine;
pub mod experimental;
pub mod media;
pub mod net;
pub mod platform;
pub mod storage;
pub mod ui;

pub use core::*;
pub use engine::*;
pub use experimental::*;
pub use media::*;
pub use net::*;
pub use platform::*;
pub use storage::*;
pub use ui::*;

pub use acceptance::{
    AcceptanceAuditor, AcceptanceEvidenceBundle, AcceptanceReleaseManager, AcceptanceReport,
    AuditEvidence, EvidenceArtifact, ExternalReleaseEvidence, PerformanceSoakTracker,
    PerformanceSummary, ScenarioCapability, ScenarioCategory, ScenarioDefinition, ScenarioEvidence,
    ScenarioMatrix, ScenarioResult, SoakSample,
};
pub use adblock::{
    AdBlockConfig, AdBlockDecision, AdBlockReason, AdBlockStats, AdBlocker, ResourceType,
};
pub use extensions::{
    ContentScriptConfig, ExtensionApproval, ExtensionError, ExtensionManager, ExtensionPackage,
    ExtensionPermission, ExtensionPermissionReview, ExtensionRecord, ExtensionStatus,
    ExtensionStorage, ExtensionWorker, GhitaExtensionManifest, SylphraExtensionManifest,
};
pub use installed_app::{
    AppDisplayMode, AppError, AppIconConfig, InstalledAppApproval, InstalledAppManager,
    InstalledAppManifest, InstalledAppRecord, InstalledAppReview, InstalledAppShortcut,
    InstalledAppWindow,
};
pub use notes::{NoteStore, QuickNote};
pub use parallel_downloader::{DownloadChunk, ParallelDownloadTask};
pub use passwords::{
    PasswordStore, SavedPassword, SystemCredential, WindowsCredentialStore, WindowsPasskeyPlatform,
};
pub use pip::PipState;
pub use reader_mode::{ReaderArticle, ReaderModeExtractor, ReaderSettings, ReaderTheme};
pub use sidebar::{PinnedApp, SidebarPanel, SidebarState};
pub use task_manager::{GovernanceSummary, ProcessTaskInfo, TaskManager};
pub use updater::{
    RepairEngine, UninstallChoice, UpdateError, UpdateFault, UpdateInstaller, UpdateManager,
    UpdateManifest, UpdatePackage, UpdateState, VersionComparer,
};
pub use web_capture::{CaptureMode, RectRegion, WebCaptureState};
pub use windows_integration::{
    BrowserNotification, CliAction, CrashReportConsent, FileAssociation, ProtocolHandler,
    WindowsIntegration,
};

pub use app_platform::{
    ApplicationDocument, CustomElementDefinition, CustomElementRegistry, HydrationReport,
    LifecycleKind, LifecycleRecord, ShadowMode, SlotAssignment,
};

pub use css_parser::{parse_css, ComputedStyle, CssRule};

pub use dynamic_render::{
    DynamicInvalidation, DynamicRenderFrame, DynamicRenderMetrics, DynamicRenderer,
};

pub use image_loader::ImageCache;

pub use javascript::JsvEngine;

pub use layout::{create_layout_tree, perform_layout, LayoutNode};

pub use live_dom::{
    DefaultAction, DispatchReport, DomEvent, EventPhase, ListenerOptions, LiveDocument, LiveNode,
    LiveNodeKind, LiveRenderState, MutationInvalidation, NodeId,
};

pub use memory_tracker::{
    BrowserMemoryEstimate, MemoryBudget, MemoryPressureLevel, MemoryReliefReport, MemoryTracker,
    TabMemoryEstimate,
};

pub use network::{fetch_url, fetch_with_cache, CacheStats, FetchResult, ResourceCache};

pub use paint::{
    build_display_list, build_display_list_with_cache, DisplayItem, DisplayList, LinkRegion, Rgba,
};

pub use parser::{parse_html, Element};

pub use performance::Profiler;
pub use performance::{DynamicFrameBudget, NavigationMetrics, PerformanceBudget};

pub use renderer::render_to_string;

pub use search::{search_web, SearchResult};

pub use storage::{
    Bookmark, BrowserSession, BrowserSettings, Cookie, CookieStore, DownloadRecord, HistoryRecord,
    LocalStorage, SessionTab, SessionTabGroup, StorageManager,
};

pub use tab::{Tab, TabManager};

pub use web_api::{
    AbortController, AbortSignal, CredentialsMode, FetchError, FetchPromiseId, FetchPromiseState,
    FetchRuntime, Headers, RedirectMode, RequestMode, ResponseType, UrlSearchParams, WebRequest,
    WebResponse, WebTimerQueue, WebUrl, XmlHttpRequest,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderStats {
    pub parse_time_ms: u64,
    pub style_time_ms: u64,
    pub layout_time_ms: u64,
    pub render_time_ms: u64,
    pub total_time_ms: u64,
    pub dom_nodes: usize,
    pub layout_nodes: usize,
}

pub struct Browser {
    tabs: TabManager,

    viewport_width: u32,
    viewport_height: u32,

    pub storage: StorageManager,

    pub cache: ResourceCache,

    pub js_engine: JsvEngine,

    pub profiler: Profiler,

    pub css_rules: Vec<CssRule>,

    pub last_render_stats: Option<RenderStats>,

    pub image_cache: ImageCache,

    pub memory_tracker: memory_tracker::MemoryTracker,

    pub resources: resource_ledger::SharedLedger,

    pressure: memory_tracker::PressureGovernor,

    pub adblocker: AdBlocker,
    pub content_control: content_control::ContentControlEngine,
    pub https_upgrade: https_upgrade::HttpsUpgradeEngine,
    pub cookie_blocker: tracking_protection::ThirdPartyCookieBlocker,

    pub process_coordinator: Option<process_coordinator::BrowserProcessCoordinator>,

    pub renderer_fallback: bool,

    pub extension_manager: extensions::ExtensionManager,

    pub app_manager: installed_app::InstalledAppManager,

    pub updater: updater::UpdateManager,

    pub win_integration: windows_integration::WindowsIntegration,

    pub acceptance: acceptance::AcceptanceReleaseManager,
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    pub fn new() -> Self {
        Self::with_storage(StorageManager::new())
    }

    pub fn new_in_memory() -> Self {
        Self::with_storage(StorageManager::in_memory())
    }

    pub fn new_with_profile(
        base_dir: impl AsRef<std::path::Path>,
        profile_name: &str,
    ) -> Result<Self, String> {
        Ok(Self::with_storage(StorageManager::for_profile(
            base_dir,
            profile_name,
        )?))
    }

    fn with_storage(storage: StorageManager) -> Self {
        let image_cache_capacity_mb = if storage.settings.image_cache_capacity_mb == 0 {
            24
        } else {
            storage.settings.image_cache_capacity_mb.clamp(8, 128)
        };
        let adblocker = AdBlocker::new(AdBlockConfig {
            enabled: storage.settings.adblock_enabled,
            cosmetic_filtering: storage.settings.adblock_cosmetic_filtering,
            disabled_domains: storage.settings.adblock_disabled_domains.clone(),
            ..Default::default()
        });

        let (extension_manager, app_manager, updater, win_integration) = match storage.storage_dir()
        {
            Some(dir) => (
                extensions::ExtensionManager::new_with_profile(dir)
                    .unwrap_or_else(|_| extensions::ExtensionManager::new_in_memory()),
                installed_app::InstalledAppManager::new_with_profile(dir)
                    .unwrap_or_else(|_| installed_app::InstalledAppManager::new_in_memory()),
                updater::UpdateManager::new_for_application(VERSION, dir).unwrap_or_else(|error| {
                    log::error!(
                        "updater initialization failed (possible interrupted update \
                         needing manual repair): {error:?}"
                    );
                    eprintln!(
                        "Warning: update system could not start ({error:?}). \
                         If updates were recently applied, the installation may \
                         need repair."
                    );
                    updater::UpdateManager::new_in_memory(VERSION)
                }),
                windows_integration::WindowsIntegration::new_with_profile(dir)
                    .unwrap_or_else(|_| windows_integration::WindowsIntegration::new_in_memory()),
            ),
            None => (
                extensions::ExtensionManager::new_in_memory(),
                installed_app::InstalledAppManager::new_in_memory(),
                updater::UpdateManager::new_in_memory(VERSION),
                windows_integration::WindowsIntegration::new_in_memory(),
            ),
        };

        Self {
            tabs: TabManager::new(),
            viewport_width: 1100,
            viewport_height: 780,
            storage,
            cache: ResourceCache::new(),
            js_engine: JsvEngine::new(),
            profiler: Profiler::new(),
            css_rules: Vec::new(),
            last_render_stats: None,
            image_cache: ImageCache::with_capacity(
                (image_cache_capacity_mb as usize).saturating_mul(1024 * 1024),
            ),
            memory_tracker: memory_tracker::MemoryTracker::new(),
            resources: {
                let ledger =
                    resource_ledger::ResourceLedger::new(resource_ledger::LedgerLimits::from_mb(
                        resource_caps::GLOBAL_SOFT_LIMIT_MB,
                        resource_caps::GLOBAL_HARD_LIMIT_MB,
                    ));
                resource_ledger::ResourceLedger::install_shared(ledger.clone());
                ledger
            },
            pressure: memory_tracker::PressureGovernor::new(),
            adblocker,
            content_control: content_control::ContentControlEngine::new(),
            https_upgrade: https_upgrade::HttpsUpgradeEngine::new(
                https_upgrade::HttpsMode::EnabledAll,
            ),
            cookie_blocker: tracking_protection::ThirdPartyCookieBlocker::new(
                tracking_protection::CookiePolicy::BlockThirdParty,
            ),
            process_coordinator: None,
            renderer_fallback: false,
            extension_manager,
            app_manager,
            updater,
            win_integration,
            acceptance: acceptance::AcceptanceReleaseManager::new(),
        }
    }

    pub fn initialize_process_architecture(&mut self) -> Result<usize, String> {
        if let Some(coordinator) = self.process_coordinator.as_ref() {
            return Ok(coordinator.native_process_count());
        }
        let coordinator = process_coordinator::BrowserProcessCoordinator::discover()?;
        let count = coordinator.native_process_count();
        self.process_coordinator = Some(coordinator);
        Ok(count)
    }

    pub fn initialize_process_architecture_from(
        &mut self,
        program: impl AsRef<std::path::Path>,
    ) -> Result<usize, String> {
        if let Some(coordinator) = self.process_coordinator.as_ref() {
            return Ok(coordinator.native_process_count());
        }
        let coordinator = process_coordinator::BrowserProcessCoordinator::start(program)?;
        let count = coordinator.native_process_count();
        self.process_coordinator = Some(coordinator);
        Ok(count)
    }

    pub fn attach_navigation_process(&mut self, tab_id: usize, url: &str) -> Result<(), String> {
        if let Some(coordinator) = self.process_coordinator.as_mut() {
            coordinator.attach_tab(tab_id, url)?;
        }
        Ok(())
    }

    pub fn render_document_in_child(
        &mut self,
        tab_id: usize,
        url: &str,
        html: &str,
        base_rules: &[css_parser::CssRule],
    ) -> Option<document::PreparedDocument> {
        let coordinator = match self.process_coordinator.as_mut() {
            Some(coordinator) => coordinator,
            None => {
                self.renderer_fallback = true;
                return None;
            }
        };
        let ledger = self.resources.clone();
        let viewport_width = self.viewport_width;
        let viewport_height = self.viewport_height;
        match coordinator.render_document(
            &ledger,
            tab_id,
            url,
            html,
            base_rules,
            viewport_width,
            viewport_height,
        ) {
            Ok(prepared) => {
                self.renderer_fallback = false;
                Some(prepared)
            }
            Err(error) => {
                log::warn!("renderer process could not render {url}: {error}");
                self.renderer_fallback = true;
                None
            }
        }
    }

    fn apply_renderer_loss(&mut self, loss: process_coordinator::RendererLoss) -> Vec<String> {
        let reason = match loss.reason {
            process_coordinator::ProcessLossReason::Crashed => "crashed",
            process_coordinator::ProcessLossReason::MemoryKilled => "reached its memory limit",
            process_coordinator::ProcessLossReason::Shed => "was shed",
        };
        let mut notes = Vec::new();
        for tab in &loss.tabs {
            let id = tab.0 as usize;
            if let Some(tab) = self.tabs.get_tab_mut(id) {
                tab.discard();
            }
            self.resources
                .release_owner(resource_ledger::OwnerId::Tab(id));
        }
        if !loss.tabs.is_empty() {
            notes.push(format!(
                "renderer for {} {reason}; {} tab(s) discarded",
                loss.origin,
                loss.tabs.len()
            ));
        }
        notes
    }

    pub fn poll_renderer_memory(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let losses = match self.process_coordinator.as_mut() {
            Some(coordinator) => {
                let ledger = self.resources.clone();
                coordinator.poll_process_memory(&ledger)
            }
            None => return notes,
        };
        for loss in losses {
            notes.extend(self.apply_renderer_loss(loss));
        }
        notes
    }

    pub fn shed_renderers(&mut self, shed_count: usize) -> Vec<String> {
        let mut scored: Vec<(String, i64)> = Vec::new();
        if let Some(coordinator) = self.process_coordinator.as_ref() {
            let active_id = self.tabs.active_tab_id();
            for (origin, process) in coordinator.renderer_origins() {
                let tabs = coordinator.tabs_for_process(process);
                let protected = tabs
                    .iter()
                    .filter_map(|tab| self.tabs.get_tab(tab.0 as usize))
                    .any(|tab| tab.is_memory_relief_protected(active_id));
                if protected {
                    continue;
                }
                let score = tabs
                    .iter()
                    .filter_map(|tab| self.tabs.get_tab(tab.0 as usize))
                    .map(|tab| tab.discard_score(active_id))
                    .max()
                    .unwrap_or(i64::MIN);
                scored.push((origin, score));
            }
        }
        scored.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let mut notes = Vec::new();
        for (origin, _) in scored.into_iter().take(shed_count) {
            let Some(coordinator) = self.process_coordinator.as_mut() else {
                break;
            };
            let ledger = self.resources.clone();
            let Some(loss) = coordinator.shed_origin(&ledger, &origin) else {
                continue;
            };
            notes.extend(self.apply_renderer_loss(loss));
        }
        notes
    }

    pub fn restore_previous_session(&mut self) -> usize {
        let session = self.storage.session().clone();
        self.tabs.restore_session(&session)
    }

    pub fn persist_session(&mut self) {
        self.storage.set_session(self.tabs.session_snapshot());
        self.storage.save();
    }

    pub fn pin_tab(&mut self, index: usize, pinned: bool) -> bool {
        let changed = self.tabs.pin_tab_by_index(index, pinned);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn toggle_tab_mute(&mut self, index: usize) -> Option<bool> {
        let muted = self.tabs.toggle_mute_by_index(index);
        if muted.is_some() {
            self.persist_session();
        }
        muted
    }

    pub fn create_tab_group(&mut self, name: &str, color: &str) -> Result<u64, String> {
        let group = self.tabs.create_group(name, color)?;
        self.persist_session();
        Ok(group)
    }

    pub fn assign_tab_group(&mut self, index: usize, group: Option<u64>) -> bool {
        let changed = self.tabs.assign_tab_to_group_by_index(index, group);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn reorder_tab(&mut self, from_index: usize, to_index: usize) -> bool {
        let changed = self.tabs.reorder_tab(from_index, to_index);
        if changed {
            self.persist_session();
        }
        changed
    }

    pub fn secure_navigation_url(&self, url: &str) -> String {
        match self.https_upgrade.evaluate_url(url) {
            https_upgrade::HttpsUpgradeResult::Upgraded { new_url } => new_url,
            https_upgrade::HttpsUpgradeResult::AlreadySecure { url }
            | https_upgrade::HttpsUpgradeResult::NonHttpScheme { url }
            | https_upgrade::HttpsUpgradeResult::ExemptLocal { url }
            | https_upgrade::HttpsUpgradeResult::InsecureAllowed { url } => url,

            https_upgrade::HttpsUpgradeResult::Invalid { .. }
            | https_upgrade::HttpsUpgradeResult::InsecureBlocked { .. } => String::new(),
        }
    }

    pub fn cookie_header_for_navigation(&self, top_level_url: &str, request_url: &str) -> String {
        let cookies = storage::cookie_header_for(self.storage.cookies(), request_url);
        if cookies.is_empty() {
            return cookies;
        }
        let request_domain = url::Url::parse(request_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        if self
            .cookie_blocker
            .should_allow_cookie(top_level_url, &request_domain)
        {
            cookies
        } else {
            String::new()
        }
    }

    pub fn permission_state(
        &self,
        origin: &str,
        permission: permissions::PermissionType,
    ) -> permissions::PermissionState {
        self.storage
            .permissions()
            .get_permission(origin, permission)
    }

    pub fn set_permission(
        &mut self,
        origin: &str,
        permission: permissions::PermissionType,
        state: permissions::PermissionState,
    ) -> Result<(), String> {
        self.storage.set_permission(origin, permission, state)
    }

    pub fn reset_permissions_for_origin(&mut self, origin: &str) -> bool {
        self.storage.reset_permissions_for_origin(origin)
    }

    pub fn load_url(&mut self, url: &str) -> Result<String, String> {
        let secure_url = self.secure_navigation_url(url);
        let url = secure_url.as_str();
        let start = std::time::Instant::now();

        let fetch_start = std::time::Instant::now();
        let fetch_result =
            network::fetch_with_cache(url, &mut self.cache, Some(self.storage.cookies_mut()))
                .map_err(|e| format!("Network error: {}", e))?;
        let fetch_time = fetch_start.elapsed().as_millis() as u64;
        self.profiler.record("fetch", fetch_time);

        let html_content = &fetch_result.body;

        let parse_start = std::time::Instant::now();
        let dom = parser::parse_html(html_content);
        let parse_time = parse_start.elapsed().as_millis() as u64;
        self.profiler.record("parse", parse_time);

        let title = extract_title_from_dom(&dom);

        let style_start = std::time::Instant::now();

        let mut page_css_rules: Vec<css_parser::CssRule> = Vec::new();
        page_css_rules.extend(css_parser::parse_css(
            &self.content_control.generate_cosmetic_css_for_origin(url),
        ));
        let style_elements = dom.find_all_tags("style");
        for style_elem in &style_elements {
            let css_text = style_elem.text.trim();
            if !css_text.is_empty() {
                let mut rules = css_parser::parse_css(css_text);
                page_css_rules.append(&mut rules);
            }
        }

        let link_elements = dom.find_all_tags("link");
        let page_domain = url::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string));
        for link_elem in &link_elements {
            if link_elem.get_attr("rel").map(|s| s.as_str()) == Some("stylesheet") {
                if let Some(href) = link_elem.get_attr("href") {
                    let css_url = resolve_url(url, href);
                    if self.adblocker.should_block_resource(
                        &css_url,
                        page_domain.as_deref(),
                        ResourceType::Style,
                    ) {
                        continue;
                    }

                    let css_domain = url::Url::parse(&css_url)
                        .ok()
                        .and_then(|parsed| parsed.host_str().map(str::to_string))
                        .unwrap_or_default();
                    let allow_cookies = self.cookie_blocker.should_allow_cookie(url, &css_domain);
                    let result = if allow_cookies {
                        network::fetch_with_cache(
                            &css_url,
                            &mut self.cache,
                            Some(self.storage.cookies_mut()),
                        )
                    } else {
                        network::fetch_with_cache(&css_url, &mut self.cache, None)
                    };
                    if let Ok(result) = result {
                        let mut rules = css_parser::parse_css(&result.body);
                        page_css_rules.append(&mut rules);
                    }
                }
            }
        }

        let all_rules: Vec<css_parser::CssRule> = self
            .css_rules
            .iter()
            .cloned()
            .chain(page_css_rules)
            .collect();

        let style_time = style_start.elapsed().as_millis() as u64;
        self.profiler.record("style", style_time);

        let layout_start = std::time::Instant::now();
        let layout_tree = layout::create_layout_tree(&dom, &all_rules, self.viewport_width);
        let layout_time = layout_start.elapsed().as_millis() as u64;
        self.profiler.record("layout", layout_time);

        if let Some(ref _root) = layout_tree {
            if let Some(tab) = self.tabs.active_tab_mut() {
                tab.set_layout(layout_tree.clone());
            }
        }

        let dom_nodes = count_elements(&dom);
        let layout_nodes = layout_tree
            .as_ref()
            .map(crate::layout::count_layout_nodes)
            .unwrap_or(0);

        let render_start = std::time::Instant::now();
        let rendered = if let Some(root) = layout_tree {
            let tr = text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
            tr.render_to_text(&root)
        } else {
            String::from("[Empty page]")
        };
        let render_time = render_start.elapsed().as_millis() as u64;
        self.profiler.record("render", render_time);

        let total_time = start.elapsed().as_millis() as u64;

        self.last_render_stats = Some(RenderStats {
            parse_time_ms: parse_time,
            style_time_ms: style_time,
            layout_time_ms: layout_time,
            render_time_ms: render_time,
            total_time_ms: total_time,
            dom_nodes,
            layout_nodes,
        });

        if let Some(tab) = self.tabs.active_tab_mut() {
            let new_entry = crate::tab::HistoryEntry::new(url.to_string(), title.clone(), &dom);
            tab.push_history(new_entry);

            tab.set_dom(dom);
            tab.title = title;
            tab.url = url.to_string();
        } else {
            self.tabs.add_tab(url, dom, &title);
        }

        let budgeted = self.sync_tab_budgets();
        if budgeted > 0 {
            log::debug!("{budgeted} bytes reconciled into the resource ledger after navigation");
        }

        Ok(rendered)
    }

    pub fn load_html(&mut self, url: &str, html_content: &str) -> Result<String, String> {
        let secure_url = self.secure_navigation_url(url);
        let url = secure_url.as_str();
        let dom = parser::parse_html(html_content);
        let title = extract_title_from_dom(&dom);

        let mut rules = self.css_rules.clone();
        rules.extend(css_parser::parse_css(
            &self.content_control.generate_cosmetic_css_for_origin(url),
        ));
        for style in dom.find_all_tags("style") {
            rules.extend(css_parser::parse_css(&style.text));
        }
        let layout_tree = layout::create_layout_tree(&dom, &rules, self.viewport_width);

        if let Some(tab) = self.tabs.active_tab_mut() {
            let new_entry = crate::tab::HistoryEntry::new(url.to_string(), title.clone(), &dom);
            tab.push_history(new_entry);

            tab.set_dom(dom);
            tab.title = title;
            tab.url = url.to_string();
            tab.set_layout(layout_tree);
        } else {
            let tab_id = self.add_tab(url, dom, &title);
            if let Some(tab) = self.tabs.get_tab_mut(tab_id) {
                tab.set_layout(layout_tree);
            }
        }

        self.sync_tab_budgets();
        Ok(self.render_current())
    }

    pub fn add_tab(&mut self, url: &str, dom: Element, title: &str) -> usize {
        self.tabs.add_tab(url, dom, title)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.active_tab()
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.active_tab_mut()
    }

    pub fn tab_by_index(&self, index: usize) -> Option<&Tab> {
        self.tabs.get_tab_by_index(index)
    }

    pub fn tab_groups(&self) -> &std::collections::HashMap<u64, tab::TabGroup> {
        self.tabs.groups()
    }

    pub fn go_back(&mut self) -> tab::HistoryNavigation {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_back()
        } else {
            tab::HistoryNavigation::Blocked
        }
    }

    pub fn go_forward(&mut self) -> tab::HistoryNavigation {
        if let Some(tab) = self.tabs.active_tab_mut() {
            tab.go_forward()
        } else {
            tab::HistoryNavigation::Blocked
        }
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.tab_count()
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport_width = width;
        self.viewport_height = height;
    }

    pub fn viewport_width(&self) -> u32 {
        self.viewport_width
    }

    pub fn viewport_height(&self) -> u32 {
        self.viewport_height
    }

    pub fn set_css(&mut self, css: &str) {
        self.css_rules = css_parser::parse_css(css);
    }

    pub fn render_current(&self) -> String {
        if let Some(tab) = self.active_tab() {
            if let Some(ref layout_root) = tab.layout {
                let tr =
                    text_renderer::TextRenderer::new(self.viewport_width, self.viewport_height);
                tr.render_to_text(layout_root)
            } else {
                let css_rules = &self.css_rules;
                match layout::create_layout_tree(&tab.dom, css_rules, self.viewport_width) {
                    Some(root) => {
                        let tr = text_renderer::TextRenderer::new(
                            self.viewport_width,
                            self.viewport_height,
                        );
                        tr.render_to_text(&root)
                    }
                    None => String::from("[Error rendering content]"),
                }
            }
        } else {
            String::from("[No active tab]")
        }
    }

    pub fn estimate_memory(&self) -> memory_tracker::BrowserMemoryEstimate {
        let tabs: Vec<&Tab> = self.tabs.iter().collect();
        memory_tracker::MemoryTracker::estimate_browser(&tabs, &self.image_cache, &self.cache)
    }

    pub fn estimate_tab_memory(&self, tab_id: usize) -> Option<memory_tracker::TabMemoryEstimate> {
        self.tabs
            .get_tab(tab_id)
            .map(memory_tracker::MemoryTracker::estimate_tab)
    }

    pub fn maybe_sleep_inactive_tab(
        &mut self,
        threshold_minutes: u32,
        sleep_delay_seconds: i64,
    ) -> Option<usize> {
        if threshold_minutes == 0 {
            return None;
        }

        let active_id = self.tabs.active_tab_id();

        let candidate = self
            .tabs
            .iter()
            .filter(|t| {
                Some(t.id) != active_id
                    && t.can_sleep()
                    && t.seconds_since_active() >= sleep_delay_seconds
                    && t.seconds_since_active() as u32 >= threshold_minutes.saturating_mul(60)
            })
            .max_by_key(|t| t.seconds_since_active());

        if let Some(tab) = candidate {
            let id = tab.id;
            if let Some(t) = self.tabs.get_tab_mut(id) {
                t.sleep();
                return Some(id);
            }
        }

        None
    }

    pub fn wake_tab(&mut self, tab_id: usize) -> tab::WakeResult {
        self.tabs
            .get_tab_mut(tab_id)
            .map(|t| t.wake())
            .unwrap_or(tab::WakeResult::NotSleeping)
    }

    pub fn is_tab_sleeping(&self, tab_id: usize) -> bool {
        self.tabs.get_tab(tab_id).is_some_and(|t| t.is_sleeping)
    }

    pub fn is_tab_discarded(&self, tab_id: usize) -> bool {
        self.tabs.get_tab(tab_id).is_some_and(|t| t.is_discarded)
    }

    pub fn tab_discard_scores(&self) -> Vec<(usize, i64)> {
        let active_id = self.tabs.active_tab_id();
        let mut scores: Vec<(usize, i64)> = self
            .tabs
            .iter()
            .map(|t| (t.id, t.discard_score(active_id)))
            .collect();
        scores.sort_by_key(|b| std::cmp::Reverse(b.1));
        scores
    }

    pub fn memory_budget(&self) -> memory_tracker::MemoryBudget {
        memory_tracker::MemoryBudget::from_mb(
            self.storage.settings.memory_soft_limit_mb,
            self.storage.settings.memory_pressure_threshold_mb,
        )
    }

    pub fn pressure_status(&mut self) -> memory_tracker::PressureReading {
        let budget = self.memory_budget();
        let sample = memory_probe::sample();
        self.resources
            .note_main_working_set(sample.process_working_set_bytes);
        let consumed = self.estimate_memory().total_bytes;
        self.pressure.observe(budget, consumed, sample.system)
    }

    pub fn pressure_level(&self) -> memory_tracker::MemoryPressureLevel {
        self.pressure.level()
    }

    pub fn pressure_reading(&self) -> Option<memory_tracker::PressureReading> {
        self.pressure.last_reading()
    }

    pub fn next_pressure_interval(&self) -> std::time::Duration {
        if self.pressure.level().relief_required() {
            resource_caps::PRESSURE_PULSE
        } else if self.memory_budget().is_enforced() {
            resource_caps::PRESSURE_HEARTBEAT
        } else {
            resource_caps::PRESSURE_IDLE_HEARTBEAT
        }
    }

    pub fn measured_working_set_bytes(&self) -> u64 {
        self.resources.real_working_set_bytes()
    }

    pub fn resource_denials(&self) -> usize {
        self.resources.totals().denials
    }

    pub fn measured_main_working_set_bytes(&self) -> u64 {
        self.resources.main_working_set_bytes()
    }

    pub fn measured_child_working_set_bytes(&self) -> u64 {
        self.resources.child_working_set_bytes()
    }

    pub fn tab_committed_bytes(&self, tab_id: usize) -> usize {
        self.resources
            .owner_bytes(resource_ledger::OwnerId::Tab(tab_id))
    }

    pub fn committed_bytes_by_tab(&self) -> Vec<(usize, usize)> {
        self.tabs
            .iter()
            .map(|tab| (tab.id, self.tab_committed_bytes(tab.id)))
            .collect()
    }

    pub fn native_renderer_count(&self) -> usize {
        self.process_coordinator
            .as_ref()
            .map(|coordinator| coordinator.native_process_count())
            .unwrap_or_default()
    }

    pub fn governance_summary(&mut self) -> task_manager::GovernanceSummary {
        let budget = self.memory_budget();
        let reading = self.pressure_status();
        let snapshot = self.resources.snapshot();
        let totals = snapshot.totals;
        let mb = |bytes: u64| bytes as f32 / resource_caps::MB as f32;
        task_manager::GovernanceSummary {
            main_rss_mb: mb(self.resources.main_working_set_bytes()),
            child_rss_mb: mb(self.resources.child_working_set_bytes()),
            native_renderer_count: self.native_renderer_count(),
            fallback_parsing: self.renderer_fallback,
            committed_mb: mb(totals.committed_bytes as u64),
            reserved_mb: mb(totals.reserved_bytes as u64),
            denied_reservations: totals.denials,
            denied_mb: mb(totals.denial_bytes as u64),
            interned_savings_mb: mb(string_pool::interned_savings_bytes() as u64),
            pressure_level: reading.level.label().to_string(),
            system_available_mb: mb(reading.system_available_bytes),
            soft_limit_mb: budget.soft_limit_bytes as f32 / resource_caps::MB as f32,
            hard_limit_mb: budget.hard_limit_bytes as f32 / resource_caps::MB as f32,
            driven_by_system: reading.driven_by_system,
            relief_required: reading.level.relief_required(),
            subsystems: snapshot
                .subsystems
                .iter()
                .map(|usage| task_manager::SubsystemCeilingReport {
                    label: usage.subsystem.label().to_string(),
                    held_mb: mb(usage.held_bytes() as u64),
                    ceiling_mb: mb(usage.ceiling_bytes as u64),
                    utilisation_percent: usage.utilisation_percent(),
                })
                .collect(),
        }
    }

    pub fn snapshot_directory(&self) -> Option<std::path::PathBuf> {
        self.storage
            .storage_dir()
            .map(|dir| dir.join("tab_snapshots"))
    }

    pub fn sync_tab_budgets(&mut self) -> usize {
        let ids: Vec<usize> = self.tabs.iter().map(|tab| tab.id).collect();
        let mut reconciled = 0usize;
        for id in ids {
            let Some(tab) = self.tabs.get_tab(id) else {
                continue;
            };
            let owner = resource_ledger::OwnerId::Tab(id);
            let dom_bytes = tab
                .estimated_dom_bytes()
                .saturating_add(tab.history_retained_bytes())
                .saturating_add(tab.compressed_snapshot_bytes());
            let layout_bytes = tab.estimated_layout_bytes();
            let runtime_bytes = tab.runtime_heap_bytes();
            reconciled = reconciled
                .saturating_add(self.resources.reconcile(
                    resource_ledger::SubsystemId::DomTree,
                    owner,
                    dom_bytes,
                ))
                .saturating_add(self.resources.reconcile(
                    resource_ledger::SubsystemId::LayoutTree,
                    owner,
                    layout_bytes,
                ))
                .saturating_add(self.resources.reconcile(
                    resource_ledger::SubsystemId::RuntimeHeap,
                    owner,
                    runtime_bytes,
                ));
        }
        reconciled
    }

    pub fn enforce_tab_budgets(&mut self) -> usize {
        let Some(directory) = self.snapshot_directory() else {
            return 0;
        };
        let active_id = self.tabs.active_tab_id();
        let ids: Vec<usize> = self.tabs.iter().map(|tab| tab.id).collect();
        let mut freed = 0usize;
        for id in ids {
            let over_budget = self.tabs.get_tab(id).is_some_and(|tab| {
                tab.estimated_live_bytes() > resource_caps::TAB_LIVE_BUDGET_BYTES
            });
            if !over_budget || Some(id) == active_id {
                continue;
            }
            let Some(tab) = self.tabs.get_tab_mut(id) else {
                continue;
            };
            if tab.is_disk_backed() || tab.is_discarded || tab.is_pinned {
                continue;
            }
            match tab.spill_to_disk(&directory) {
                Ok(released) => freed = freed.saturating_add(released),
                Err(error) => log::warn!("tab {id} could not move to disk backing: {error}"),
            }
        }
        if freed > 0 {
            self.sync_tab_budgets();
        }
        freed
    }

    pub fn check_memory_pressure(
        &mut self,
        memory_threshold_mb: u32,
        min_tabs_to_keep: usize,
    ) -> Option<usize> {
        self.relieve_memory_pressure(
            memory_tracker::MemoryBudget::from_mb(memory_threshold_mb, memory_threshold_mb),
            min_tabs_to_keep,
        )
        .discarded_tabs
        .first()
        .copied()
    }

    fn trim_caches(&mut self, stale_seconds: u64) -> usize {
        let before = self
            .cache
            .total_bytes()
            .saturating_add(self.image_cache.memory_usage());
        self.cache.evict_expired();
        self.image_cache
            .evict_stale(std::time::Duration::from_secs(stale_seconds));
        self.cache.set_max_size(resource_caps::RESOURCE_CACHE_BYTES);
        let image_limit_mb = if self.storage.settings.image_cache_capacity_mb == 0 {
            resource_caps::IMAGE_CACHE_DEFAULT_MB
        } else {
            self.storage.settings.image_cache_capacity_mb.clamp(
                resource_caps::IMAGE_CACHE_MIN_MB,
                resource_caps::IMAGE_CACHE_MAX_MB,
            )
        };
        self.image_cache
            .set_capacity((image_limit_mb as usize).saturating_mul(resource_caps::MB));
        let after = self
            .cache
            .total_bytes()
            .saturating_add(self.image_cache.memory_usage());
        self.resources.reconcile(
            resource_ledger::SubsystemId::ImageCache,
            resource_ledger::OwnerId::Global,
            self.image_cache.memory_usage(),
        );
        self.resources.reconcile(
            resource_ledger::SubsystemId::ResourceCache,
            resource_ledger::OwnerId::Global,
            self.cache.total_bytes(),
        );
        before.saturating_sub(after)
    }

    pub fn relieve_memory_pressure(
        &mut self,
        budget: memory_tracker::MemoryBudget,
        min_tabs_to_keep: usize,
    ) -> memory_tracker::MemoryReliefReport {
        let before_bytes = self.estimate_memory().total_bytes;
        let sample = memory_probe::sample();
        self.resources
            .note_main_working_set(sample.process_working_set_bytes);
        let reading = self.pressure.observe(budget, before_bytes, sample.system);
        let level = reading.level;
        let mut report = memory_tracker::MemoryReliefReport {
            level,
            before_bytes,
            after_bytes: before_bytes,
            measured_working_set_bytes: sample.process_working_set_bytes,
            system_available_bytes: sample.system.available_bytes,
            ..memory_tracker::MemoryReliefReport::default()
        };
        if !level.relief_required() {
            return report;
        }

        report.cache_bytes_freed = self.trim_caches(resource_caps::RELIEF_STALE_CACHE_SECONDS);
        report.after_bytes = report.after_bytes.saturating_sub(report.cache_bytes_freed);

        let target = if level.shed_immediately() {
            budget.hard_limit_bytes
        } else {
            budget.soft_limit_bytes
        };
        let pass_cap = self
            .tabs
            .iter()
            .count()
            .saturating_mul(resource_caps::RELIEF_PASS_TAB_PERCENT)
            .max(resource_caps::RELIEF_KEEP_MIN_TABS)
            / 100
            + 1;
        let active_id = self.tabs.active_tab_id();

        for _ in 0..pass_cap {
            if report.after_bytes < target {
                break;
            }
            let candidates: Vec<(usize, i64)> = self
                .tabs
                .iter()
                .filter(|tab| !tab.is_memory_relief_protected(active_id) && tab.can_sleep())
                .map(|tab| (tab.id, tab.seconds_since_active()))
                .collect();
            let Some((tab_id, _)) = candidates.into_iter().max_by_key(|(_, inactive)| *inactive)
            else {
                break;
            };
            let freed = match self.tabs.get_tab_mut(tab_id) {
                Some(tab) => tab.sleep(),
                None => break,
            };
            report.slept_tabs.push(tab_id);
            report.after_bytes = report.after_bytes.saturating_sub(freed);
            report.relief_passes += 1;
        }

        if level.shed_immediately() && report.after_bytes >= budget.hard_limit_bytes {
            let mut discarded_this_call = 0usize;
            while report.after_bytes >= budget.hard_limit_bytes && discarded_this_call < 3 {
                let live_tabs = self.tabs.iter().filter(|tab| !tab.is_discarded).count();
                if live_tabs <= min_tabs_to_keep.max(resource_caps::RELIEF_KEEP_MIN_TABS) {
                    break;
                }
                let candidates: Vec<(usize, i64)> = self
                    .tabs
                    .iter()
                    .filter(|tab| !tab.is_memory_relief_protected(active_id))
                    .map(|tab| (tab.id, tab.discard_score(active_id)))
                    .collect();
                let Some((tab_id, _)) = candidates.into_iter().max_by_key(|(_, score)| *score)
                else {
                    break;
                };
                let freed = match self.tabs.get_tab_mut(tab_id) {
                    Some(tab) => tab.discard(),
                    None => break,
                };
                self.resources
                    .release_owner(resource_ledger::OwnerId::Tab(tab_id));
                report.discarded_tabs.push(tab_id);
                report.after_bytes = report.after_bytes.saturating_sub(freed);
                report.relief_passes += 1;
                discarded_this_call += 1;
            }
        }

        if level.shed_immediately() && report.after_bytes >= budget.soft_limit_bytes {
            for note in self.shed_renderers(1) {
                log::info!("{note}");
            }
        }

        report.released_ledger_bytes = self.sync_tab_budgets();
        report
    }

    pub fn discard_least_important_tab(&mut self) -> Option<usize> {
        let active_id = self.tabs.active_tab_id();
        let scores = self.tab_discard_scores();
        for (tab_id, score) in scores {
            if score >= 0 {
                if let Some(tab) = self.tabs.get_tab_mut(tab_id) {
                    if tab.is_memory_relief_protected(active_id) {
                        continue;
                    }
                    tab.discard();
                    return Some(tab_id);
                }
            }
        }
        None
    }

    pub fn undiscard_tab(&mut self, tab_id: usize) -> Option<String> {
        self.tabs.get_tab_mut(tab_id).and_then(|t| t.undiscard())
    }

    pub fn status_string(&self) -> String {
        let cache_stats = self.cache.stats();

        format!(
            "Viewport: {}x{} | {} | Cookies: {} | Tabs: {}",
            self.viewport_width,
            self.viewport_height,
            cache_stats,
            self.storage.cookie_count(),
            self.tabs.tab_count(),
        )
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.storage.set_session(self.tabs.session_snapshot());
    }
}

fn extract_title_from_dom(dom: &Element) -> String {
    if let Some(title_elem) = dom.find_tag("title") {
        return title_elem.text.trim().to_string();
    }
    if let Some(h1_elem) = dom.find_tag("h1") {
        return h1_elem.text.trim().to_string();
    }
    "Untitled Page".to_string()
}

fn count_elements(element: &Element) -> usize {
    let mut count = 0usize;
    let mut stack: Vec<&Element> = vec![element];
    while let Some(node) = stack.pop() {
        count = count.saturating_add(1);
        stack.extend(node.children.iter());
    }
    count
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }

    if let Ok(base_url) = url::Url::parse(base) {
        if relative.starts_with("//") {
            return format!("{}:{}", base_url.scheme(), relative);
        }

        if let Ok(resolved) = base_url.join(relative) {
            return resolved.as_str().to_string();
        }
    }

    let base_without_query = base.split('?').next().unwrap_or(base);
    let base_path = base_without_query
        .rsplit('/')
        .next()
        .unwrap_or(base_without_query);
    let base_dir = base_without_query
        .strip_suffix(base_path)
        .and_then(|dir| base.get(..dir.len()))
        .unwrap_or(base_without_query);

    if relative.starts_with('/') {
        if let Ok(base_url) = url::Url::parse(base) {
            let origin = format!(
                "{}://{}",
                base_url.scheme(),
                base_url.host_str().unwrap_or("")
            );
            if let Some(port) = base_url.port() {
                return format!("{}:{}{}", origin, port, relative);
            }
            return format!("{}{}", origin, relative);
        }
        return relative.to_string();
    }

    format!("{}{}", base_dir, relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_new() {
        let browser = Browser::new();
        assert_eq!(browser.tab_count(), 0);
        assert!(browser.active_tab().is_none());
    }

    #[test]
    fn test_browser_load_html() {
        let mut browser = Browser::new();
        let html = "<html><body><h1>Hello</h1></body></html>";
        let _ = browser.load_html("https://example.com", html);

        assert_eq!(browser.tab_count(), 1);
        assert!(browser.active_tab().is_some());
        assert_eq!(browser.active_tab().unwrap().url, "https://example.com");
    }

    #[test]
    fn test_browser_render() {
        let mut browser = Browser::new();
        let _ = browser.load_html(
            "https://example.com",
            "<html><body><h1>Welcome</h1></body></html>",
        );
        let rendered = browser.render_current();
        assert!(!rendered.is_empty());
        assert!(rendered.contains("Welcome"));
    }

    #[test]
    fn test_browser_with_css() {
        let mut browser = Browser::new();
        browser.set_css("h1 { color: red; font-size: 24px; }");
        let _ = browser.load_html(
            "https://example.com",
            "<html><body><h1>Styled</h1></body></html>",
        );
        let rendered = browser.render_current();
        assert!(rendered.contains("Styled"));
    }

    #[test]
    fn test_browser_tab_switching() {
        let mut browser = Browser::new();
        let _ = browser.load_html("https://a.com", "<html><body><h1>Page A</h1></body></html>");
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>Page B</h1></body></html>"),
            "Page B",
        );

        assert_eq!(browser.tab_count(), 2);

        assert_eq!(browser.active_tab().unwrap().url, "https://b.com");
    }

    #[test]
    fn test_extract_title() {
        let dom =
            parser::parse_html("<html><head><title>My Page</title></head><body></body></html>");
        assert_eq!(extract_title_from_dom(&dom), "My Page");
    }

    #[test]
    fn test_browser_maybe_sleep_disabled_when_threshold_zero() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );

        browser.storage.settings.memory_saver_threshold_minutes = 0;
        let result = browser.maybe_sleep_inactive_tab(0, 0);
        assert!(result.is_none());

        assert!(!browser.is_tab_sleeping(1));
        assert!(!browser.is_tab_sleeping(2));
    }

    #[test]
    fn test_browser_wake_sleeping_tab() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://example.com",
                "<html><body><h1>Test</h1></body></html>",
            )
            .unwrap();

        if let Some(tab) = browser.tabs.get_tab_mut(1) {
            tab.sleep();
        }
        assert!(browser.is_tab_sleeping(1));

        let result = browser.wake_tab(1);
        assert!(
            matches!(result, crate::tab::WakeResult::RestoredFromCache),
            "Should restore from compressed DOM"
        );
        assert!(!browser.is_tab_sleeping(1));
    }

    #[test]
    fn test_browser_wake_non_sleeping_tab_returns_not_sleeping() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://example.com",
                "<html><body><h1>Test</h1></body></html>",
            )
            .unwrap();

        let result = browser.wake_tab(1);
        assert!(
            matches!(result, crate::tab::WakeResult::NotSleeping),
            "Should return NotSleeping for non-sleeping tab"
        );
    }

    #[test]
    fn test_sleep_wake_memory_savings() {
        let mut browser = Browser::new();

        let html = format!(
            "<html><body>{}</body></html>",
            (0..100)
                .map(|i| format!("<p>Paragraph {} with some text content</p>", i))
                .collect::<String>()
        );
        browser.load_html("https://example.com", &html).unwrap();

        let mem_before = browser.estimate_memory();
        let tab_mem_before = browser.estimate_tab_memory(1).unwrap();
        assert!(
            tab_mem_before.dom_bytes > 0,
            "Tab should use DOM memory before sleep"
        );

        if let Some(tab) = browser.tabs.get_tab_mut(1) {
            let freed = tab.sleep();
            assert!(freed > 0, "Sleep should free bytes");
        }

        let mem_after = browser.estimate_memory();
        let tab_mem_after = browser.estimate_tab_memory(1).unwrap();

        assert!(
            tab_mem_after.dom_bytes < tab_mem_before.dom_bytes,
            "DOM memory should decrease after sleep: before={}, after={}",
            tab_mem_before.dom_bytes,
            tab_mem_after.dom_bytes
        );
        assert!(
            mem_after.total_bytes < mem_before.total_bytes,
            "Total memory should decrease after sleep: before={}, after={}",
            mem_before.total_bytes,
            mem_after.total_bytes
        );
    }

    #[test]
    fn test_discard_scores_sorted_descending() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();

        let pinned_dom = parser::parse_html("<html><body><h1>Pinned</h1></body></html>");
        let pinned_id = browser.add_tab("https://pinned.com", pinned_dom, "Pinned");
        if let Some(t) = browser.tabs.get_tab_mut(pinned_id) {
            t.is_pinned = true;
        }

        let normal_dom = parser::parse_html("<html><body><h1>Normal</h1></body></html>");
        browser.add_tab("https://normal.com", normal_dom, "Normal");

        let scores = browser.tab_discard_scores();
        assert_eq!(scores.len(), 3);

        for i in 1..scores.len() {
            assert!(
                scores[i - 1].1 >= scores[i].1,
                "Scores should be sorted descending"
            );
        }
    }

    #[test]
    fn test_check_memory_pressure_disabled_when_few_tabs() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();

        let result = browser.check_memory_pressure(1, 2);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_memory_pressure_disabled_when_under_threshold() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );

        let result = browser.check_memory_pressure(100000, 2);
        assert!(result.is_none());
    }

    #[test]
    fn test_discard_least_important_tab() {
        let mut browser = Browser::new();
        browser
            .load_html(
                "https://active.com",
                "<html><body><h1>Active</h1></body></html>",
            )
            .unwrap();

        let tab1_id = 1usize;

        let discardable_dom = parser::parse_html("<html><body><h1>Old</h1></body></html>");
        let tab2_id = browser.add_tab("https://old.com", discardable_dom, "Old");

        if let Some(t) = browser.tabs.get_tab_mut(tab1_id) {
            t.last_active_timestamp -= 7200;
        }

        let discarded = browser.discard_least_important_tab();
        assert!(discarded.is_some());

        let discarded_id = discarded.unwrap();
        assert_eq!(
            discarded_id, tab1_id,
            "Should discard the most inactive tab (tab1)"
        );

        assert!(browser.is_tab_discarded(discarded_id));

        assert!(!browser.is_tab_discarded(tab2_id));
    }

    #[test]
    fn test_undiscard_tab() {
        let mut browser = Browser::new();
        browser
            .load_html("https://a.com", "<html><body><h1>A</h1></body></html>")
            .unwrap();
        let tab1_id = 1usize;
        let _tab2_id = browser.add_tab(
            "https://b.com",
            parser::parse_html("<html><body><h1>B</h1></body></html>"),
            "B",
        );

        if let Some(t) = browser.tabs.get_tab_mut(tab1_id) {
            t.last_active_timestamp -= 7200;
        }

        let discarded = browser.discard_least_important_tab();
        assert!(discarded.is_some());
        assert_eq!(discarded.unwrap(), tab1_id);
        assert!(browser.is_tab_discarded(tab1_id));

        let url = browser.undiscard_tab(tab1_id);
        assert_eq!(url, Some("https://a.com".to_string()));
        assert!(!browser.is_tab_discarded(tab1_id));
    }

    #[test]
    fn test_memory_settings_defaults() {
        let tmp = std::env::temp_dir().join(format!("sylphra_test_{}", std::process::id()));
        let _ = std::fs::remove_file(tmp.join("storage.json"));

        let browser = Browser::new();
        assert!(browser.storage.settings.tab_memory_saver);
        assert_eq!(browser.storage.settings.memory_saver_threshold_minutes, 5);
        assert_eq!(browser.storage.settings.memory_soft_limit_mb, 400);
        assert_eq!(browser.storage.settings.memory_pressure_threshold_mb, 500);
        assert_eq!(browser.storage.settings.image_cache_capacity_mb, 24);
        assert_eq!(browser.image_cache.capacity(), 24 * 1024 * 1024);
        assert_eq!(browser.cache.max_size, 32 * 1024 * 1024);
    }

    #[test]
    fn moderate_pressure_sleeps_but_does_not_discard() {
        let mut browser = Browser::new_in_memory();
        let html = format!(
            "<main>{}</main>",
            (0..1_000)
                .map(|index| format!("<p>paragraph {index} with retained content</p>"))
                .collect::<String>()
        );
        for index in 0..4 {
            browser
                .load_html(&format!("https://site{index}.test"), &html)
                .unwrap();
            if let Some(tab) = browser.active_tab_mut() {
                tab.last_active_timestamp -= 3_600 + index as i64;
            }
        }
        let before = browser.estimate_memory().total_bytes;
        let report = browser.relieve_memory_pressure(
            memory_tracker::MemoryBudget::from_bytes(before.saturating_sub(1), before + 1),
            2,
        );
        assert_eq!(report.level, memory_tracker::MemoryPressureLevel::Moderate);
        assert!(report.slept_tabs.len() <= 1);
        assert!(report.discarded_tabs.is_empty());
    }

    #[test]
    fn critical_pressure_never_discards_protected_tabs() {
        let mut browser = Browser::new_in_memory();
        let html = format!("<main>{}</main>", "<p>large tab content</p>".repeat(2_000));
        let mut ids = Vec::new();
        for index in 0..6 {
            let id = browser.add_tab(
                &format!("https://site{index}.test"),
                parser::parse_html(&html),
                "Memory",
            );
            if let Some(tab) = browser.active_tab_mut() {
                tab.last_active_timestamp -= 7_200 + index as i64;
            }
            ids.push(id);
        }
        let pinned = ids[0];
        let audible = ids[1];
        browser.tabs.get_tab_mut(pinned).unwrap().is_pinned = true;
        browser.tabs.get_tab_mut(audible).unwrap().is_audible = true;
        let active = browser.tabs.active_tab_id().unwrap();

        let report =
            browser.relieve_memory_pressure(memory_tracker::MemoryBudget::from_bytes(1, 2), 2);
        for id in report.discarded_tabs {
            assert_ne!(id, active);
            assert_ne!(id, pinned);
            assert_ne!(id, audible);
        }
        assert!(!browser.is_tab_discarded(active));
        assert!(!browser.is_tab_discarded(pinned));
        assert!(!browser.is_tab_discarded(audible));
    }

    #[test]
    fn test_image_rendering_pipeline() {
        let html = r#"<html><body>
            <h1>Image Test</h1>
            <img src="https://example.com/photo.jpg" alt="Photo" width="200" height="150"/>
            <p>Some text below the image</p>
        </body></html>"#;

        let dom = parser::parse_html(html);
        let css_rules: Vec<css_parser::CssRule> = Vec::new();
        let layout_tree = layout::create_layout_tree(&dom, &css_rules, 800).unwrap();

        let image_cache = crate::image_loader::ImageCache::new();
        let display_list =
            crate::paint::build_display_list_with_cache(&layout_tree, Some(&image_cache));

        let has_pending = display_list.items.iter().any(|item| {
            matches!(item, crate::paint::DisplayItem::PendingImage { url, .. }
                if url == "https://example.com/photo.jpg")
        });
        assert!(
            has_pending,
            "Display list should contain PendingImage for the <img> tag"
        );

        let has_cached = display_list
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::Image { cached: true, .. }));
        assert!(!has_cached, "No image should be cached yet");

        let mut cache = crate::image_loader::ImageCache::new();
        cache.add(
            "https://example.com/photo.jpg".to_string(),
            crate::image_loader::Image::new("https://example.com/photo.jpg", 200, 150)
                .with_alt("Photo"),
        );

        assert!(!cache.is_decoded("https://example.com/photo.jpg"));

        let img = cache.get("https://example.com/photo.jpg").unwrap();
        assert_eq!(img.width, 200);
        assert_eq!(img.height, 150);
        assert_eq!(img.alt_text, "Photo");
        assert!(
            !img.loaded,
            "Image should not be marked loaded until decoded"
        );

        let display_list2 = crate::paint::build_display_list_with_cache(&layout_tree, Some(&cache));
        let has_cached2 = display_list2
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::Image { cached: true, .. }));
        assert!(
            !has_cached2,
            "Image still not decoded — should be PendingImage"
        );
    }
}
