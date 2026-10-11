use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2Environment, CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2,
    ICoreWebView2Controller, ICoreWebView2Controller2,
    ICoreWebView2CreateCoreWebView2ControllerCompletedHandler, ICoreWebView2Environment,
    ICoreWebView2Environment10, ICoreWebView2_3, ICoreWebView2_7, COREWEBVIEW2_COLOR,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{
    CoTaskMemPWSTR, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, WebResourceRequestedEventHandler,
};
use windows_wv2::core::{Interface, PCWSTR, PWSTR};
use windows_wv2::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows_wv2::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
use windows_wv2::Win32::System::Threading::GetCurrentProcessId;
use windows_wv2::Win32::System::WinRT::EventRegistrationToken;
use windows_wv2::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use crate::compat_backend::WebView2HostPolicy;
use crate::compat_governance::CompatLoadReport;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
pub const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_URI_UTF16_UNITS: isize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceHint {
    Document,
    Script,
    Style,
    Image,
    Font,
    Media,
    Fetch,
    Other,
}

pub fn resource_hint(uri: &str) -> ResourceHint {
    let target = uri.split(['?', '#']).next().unwrap_or(uri);
    let path = match target.find("://") {
        Some(index) => {
            let authority = &target[index + 3..];
            match authority.find('/') {
                Some(separator) => &authority[separator..],
                None => "",
            }
        }
        None => target,
    };
    let name = path.rsplit('/').next().unwrap_or("");
    let extension = match name.rfind('.') {
        Some(index) => name[index + 1..].to_ascii_lowercase(),
        None => String::new(),
    };
    match extension.as_str() {
        "" => {
            if name.is_empty() {
                ResourceHint::Document
            } else {
                ResourceHint::Other
            }
        }
        "js" | "mjs" | "cjs" => ResourceHint::Script,
        "css" => ResourceHint::Style,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg" => {
            ResourceHint::Image
        }
        "woff" | "woff2" | "ttf" | "otf" | "eot" => ResourceHint::Font,
        "mp4" | "webm" | "m4v" | "mov" | "mp3" | "m4a" | "aac" | "ogg" | "opus" | "wav" => {
            ResourceHint::Media
        }
        "json" | "txt" | "html" | "htm" | "xml" => ResourceHint::Fetch,
        _ => ResourceHint::Other,
    }
}

pub type ResourceBlocker = Arc<dyn Fn(&str, &str, ResourceHint) -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Frame {
    fn rect(self) -> RECT {
        RECT {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

pub struct CompatTabSurface {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    last_used: Instant,
}

impl Drop for CompatTabSurface {
    fn drop(&mut self) {
        let _ = unsafe { self.controller.Close() };
    }
}

struct ControllerGuard(Option<ICoreWebView2Controller>);

impl ControllerGuard {
    fn new(controller: ICoreWebView2Controller) -> Self {
        Self(Some(controller))
    }

    fn reference(&self) -> Result<&ICoreWebView2Controller, String> {
        self.0
            .as_ref()
            .ok_or_else(|| "compat controller is already consumed".to_string())
    }

    fn take(&mut self) -> Option<ICoreWebView2Controller> {
        self.0.take()
    }
}

impl Drop for ControllerGuard {
    fn drop(&mut self) {
        if let Some(controller) = self.0.take() {
            let _ = unsafe { controller.Close() };
        }
    }
}

impl CompatTabSurface {
    fn navigate(&self, url: &str) -> Result<(), String> {
        let encoded = CoTaskMemPWSTR::from(url);
        unsafe {
            self.webview
                .Navigate(*encoded.as_ref().as_pcwstr())
                .map_err(|error| error.to_string())
        }
    }

    fn reload(&self) -> Result<(), String> {
        unsafe { self.webview.Reload().map_err(|error| error.to_string()) }
    }

    fn browser_process_id(&self) -> Option<u32> {
        let runtime = self.webview.cast::<ICoreWebView2_3>().ok()?;
        let mut pid = 0u32;
        unsafe { runtime.BrowserProcessId(&mut pid).ok()? };
        if pid == 0 {
            None
        } else {
            Some(pid)
        }
    }

    fn set_foreground(&mut self) {
        self.last_used = Instant::now();
        unsafe {
            let _ = self.controller.SetIsVisible(true);
        }
        if let Ok(runtime) = self.webview.cast::<ICoreWebView2_7>() {
            let _ = unsafe { runtime.Resume() };
        }
    }

    fn set_background(&mut self, suspend: bool) {
        unsafe {
            let _ = self.controller.SetIsVisible(false);
        }
        if suspend {
            if let Ok(runtime) = self.webview.cast::<ICoreWebView2_7>() {
                let _ = unsafe { runtime.TrySuspend(None) };
            }
        }
    }
}

#[derive(Default)]
struct ControllerHandoff {
    waiting: bool,
    delivered: Option<Option<ICoreWebView2Controller>>,
}

type ControllerMailbox = Rc<RefCell<ControllerHandoff>>;

struct SurfaceRequest {
    parent_hwnd: isize,
    frame: Frame,
    url: String,
    incognito: bool,
    blocker: ResourceBlocker,
    mailbox: ControllerMailbox,
    deadline: Instant,
}

fn new_mailbox() -> ControllerMailbox {
    Rc::new(RefCell::new(ControllerHandoff::default()))
}

fn create_is_outstanding(mailbox: &ControllerMailbox) -> bool {
    let handoff = mailbox.borrow();
    handoff.waiting || handoff.delivered.is_some()
}

fn arm_create(mailbox: &ControllerMailbox) {
    let mut handoff = mailbox.borrow_mut();
    handoff.waiting = true;
    handoff.delivered = None;
}

fn deliver_controller(
    mailbox: &ControllerMailbox,
    controller: Option<ICoreWebView2Controller>,
) -> Option<ICoreWebView2Controller> {
    let mut handoff = mailbox.borrow_mut();
    if !handoff.waiting {
        return controller;
    }
    handoff.waiting = false;
    handoff.delivered = Some(controller);
    None
}

fn take_controller(mailbox: &ControllerMailbox) -> Option<Option<ICoreWebView2Controller>> {
    mailbox.borrow_mut().delivered.take()
}

fn abandon_mailbox(mailbox: &ControllerMailbox) -> Option<ICoreWebView2Controller> {
    let mut handoff = mailbox.borrow_mut();
    handoff.waiting = false;
    handoff.delivered.take().flatten()
}

pub struct WebView2Host {
    environment: Option<ICoreWebView2Environment>,
    environment_receiver: Option<Receiver<Option<ICoreWebView2Environment>>>,
    environment_deadline: Option<Instant>,
    environment_error: Option<String>,
    surfaces: HashMap<usize, CompatTabSurface>,
    requests: HashMap<usize, SurfaceRequest>,
    user_data_folder: Option<String>,
    policy: WebView2HostPolicy,
    active_tab: Option<usize>,
    window_probe: Option<(String, i32)>,
}

impl Default for WebView2Host {
    fn default() -> Self {
        Self::new()
    }
}

impl WebView2Host {
    pub fn new() -> Self {
        Self {
            environment: None,
            environment_receiver: None,
            environment_deadline: None,
            environment_error: None,
            surfaces: HashMap::new(),
            requests: HashMap::new(),
            user_data_folder: None,
            policy: WebView2HostPolicy::from_caps(),
            active_tab: None,
            window_probe: None,
        }
    }

    pub fn set_user_data_folder(&mut self, path: Option<String>) {
        if self.environment.is_none() && self.environment_receiver.is_none() {
            self.user_data_folder = path;
        }
    }

    pub fn availability(&self) -> Option<&str> {
        self.environment_error.as_deref()
    }

    pub fn set_window_probe(&mut self, title_suffix: &str, chrome_logical_height: i32) {
        self.window_probe = Some((title_suffix.to_string(), chrome_logical_height));
    }

    pub fn policy(&self) -> WebView2HostPolicy {
        self.policy
    }

    pub fn live_surface_count(&self) -> usize {
        self.surfaces.len()
    }

    pub fn live_tab_ids(&self) -> Vec<usize> {
        self.surfaces.keys().copied().collect()
    }

    fn most_recently_used(&self) -> Vec<usize> {
        let mut ordered: Vec<(Instant, usize)> = self
            .surfaces
            .iter()
            .map(|(tab_id, surface)| (surface.last_used, *tab_id))
            .collect();
        ordered.sort_unstable_by(|left, right| right.cmp(left));
        ordered.into_iter().map(|(_, tab_id)| tab_id).collect()
    }

    fn outstanding_creates(&self) -> usize {
        self.requests
            .iter()
            .filter(|(_, request)| create_is_outstanding(&request.mailbox))
            .count()
    }

    pub fn browser_process_ids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self
            .surfaces
            .values()
            .filter_map(CompatTabSurface::browser_process_id)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    pub fn set_active_tab(&mut self, active_tab_id: Option<usize>) {
        self.active_tab = active_tab_id;
        let live = self.most_recently_used();
        let suspend_backgrounds = self.policy.should_suspend_background(live.len());
        let demotions = self
            .policy
            .foreground_demotions(&live, active_tab_id.unwrap_or(usize::MAX));
        for tab_id in live {
            let Some(surface) = self.surfaces.get_mut(&tab_id) else {
                continue;
            };
            if Some(tab_id) == active_tab_id {
                surface.set_foreground();
            } else {
                surface.set_background(suspend_backgrounds || demotions.contains(&tab_id));
            }
        }
    }

    fn least_recently_used(&self, exempt: &[usize]) -> Vec<usize> {
        let mut ordered: Vec<(Instant, usize)> = self
            .surfaces
            .iter()
            .filter(|(tab_id, _)| !exempt.contains(tab_id))
            .map(|(tab_id, surface)| (surface.last_used, *tab_id))
            .collect();
        ordered.sort_unstable();
        ordered.into_iter().map(|(_, tab_id)| tab_id).collect()
    }

    fn recycle_over_cap(&mut self, exempt: usize) -> Vec<usize> {
        let total = self.surfaces.len() + self.requests.len();
        if self.policy.controllers_to_recycle(total) == 0 {
            return Vec::new();
        }
        let order = self.least_recently_used(&[exempt, self.active_tab.unwrap_or(usize::MAX)]);
        let mut recycled = Vec::new();
        for tab_id in self.policy.recycle_plan(total, &order) {
            if self.surfaces.remove(&tab_id).is_some() {
                recycled.push(tab_id);
            }
        }
        recycled
    }

    fn trim_requests_to_capacity(&mut self, protected: usize) {
        let allowed = self
            .policy
            .max_concurrent
            .saturating_sub(self.surfaces.len());
        while self.requests.len() > allowed {
            let oldest = self
                .requests
                .iter()
                .filter(|(tab_id, _)| **tab_id != protected)
                .min_by_key(|(_, request)| request.deadline)
                .map(|(tab_id, _)| *tab_id);
            let Some(tab_id) = oldest else {
                break;
            };
            self.abandon_request(tab_id);
        }
    }

    pub fn pending_surface_count(&self) -> usize {
        self.requests.len()
    }

    pub fn is_active(&self) -> bool {
        !self.surfaces.is_empty() || !self.requests.is_empty()
    }

    pub fn is_tab_active(&self, tab_id: usize) -> bool {
        self.surfaces.contains_key(&tab_id) || self.requests.contains_key(&tab_id)
    }

    pub fn request_surface(
        &mut self,
        tab_id: usize,
        surface: &WindowSurface,
        url: &str,
        incognito: bool,
        blocker: ResourceBlocker,
    ) -> Result<(), String> {
        if let Some(error) = &self.environment_error {
            return Err(error.clone());
        }
        self.begin_environment()?;
        let frame = Frame {
            left: surface.left,
            top: surface.top,
            right: surface.right,
            bottom: surface.bottom,
        };
        if let Some(existing) = self.requests.get_mut(&tab_id) {
            if create_is_outstanding(&existing.mailbox) {
                existing.parent_hwnd = surface.parent_hwnd;
                existing.frame = frame;
                existing.url = url.to_string();
                existing.incognito = incognito;
                existing.blocker = blocker;
                existing.deadline = Instant::now() + REQUEST_TIMEOUT;
                return Ok(());
            }
            self.abandon_request(tab_id);
        }
        self.surfaces.remove(&tab_id);
        self.requests.insert(
            tab_id,
            SurfaceRequest {
                parent_hwnd: surface.parent_hwnd,
                frame,
                url: url.to_string(),
                incognito,
                blocker,
                mailbox: new_mailbox(),
                deadline: Instant::now() + REQUEST_TIMEOUT,
            },
        );
        for recycled in self.recycle_over_cap(tab_id) {
            log::info!("compat surface {recycled} recycled to honour the live cap");
        }
        self.trim_requests_to_capacity(tab_id);
        if self.environment.is_some() {
            self.begin_controller(tab_id)?;
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let now = Instant::now();

        let environment_ready = match &self.environment_receiver {
            Some(receiver) => receiver.try_recv().ok(),
            None => None,
        };
        if let Some(result) = environment_ready {
            self.environment_receiver = None;
            self.environment_deadline = None;
            match result {
                Some(environment) => {
                    self.environment = Some(environment);
                    let pending: Vec<usize> = self.requests.keys().copied().collect();
                    for tab_id in pending {
                        if let Some(request) = self.requests.get_mut(&tab_id) {
                            request.deadline = now + REQUEST_TIMEOUT;
                        }
                        if let Err(error) = self.begin_controller(tab_id) {
                            notes.push(error);
                        }
                    }
                    notes.push("compat environment ready".to_string());
                }
                None => {
                    notes.push("compat environment creation failed".to_string());
                    self.fail_environment("WebView2 environment creation failed".to_string());
                }
            }
        } else if self.environment_receiver.is_some()
            && self
                .environment_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            notes.push("compat environment creation timed out".to_string());
            self.fail_environment("WebView2 environment creation timed out".to_string());
        }

        let environment_in_flight = self.environment.is_none()
            && self.environment_receiver.is_some()
            && self.environment_error.is_none();
        if !environment_in_flight {
            for tab_id in expired_requests(&self.requests, now) {
                self.abandon_request(tab_id);
                notes.push(format!("compat surface {tab_id} timed out"));
            }
        }

        let mut completed: Vec<(usize, Option<ICoreWebView2Controller>)> = Vec::new();
        for (tab_id, request) in self.requests.iter() {
            if let Some(controller) = take_controller(&request.mailbox) {
                completed.push((*tab_id, controller));
            }
        }

        for (tab_id, controller) in completed {
            let Some(request) = self.requests.remove(&tab_id) else {
                if let Some(controller) = controller {
                    let _ = unsafe { controller.Close() };
                }
                continue;
            };
            let Some(controller) = controller else {
                notes.push(format!("compat controller {tab_id} was not created"));
                continue;
            };
            let mut guard = ControllerGuard::new(controller);
            let visible = Some(tab_id) == self.active_tab;
            let frame = self.fresh_frame(request.frame);
            let outcome = match self.environment.as_ref() {
                Some(environment) => build_surface(
                    &mut guard,
                    frame,
                    &request.url,
                    environment,
                    &request.blocker,
                    visible,
                ),
                None => Err("compat environment is missing".to_string()),
            };
            match outcome {
                Ok(surface) => {
                    self.surfaces.insert(tab_id, surface);
                    notes.push(format!("compat surface {tab_id} attached"));
                    if visible {
                        let live = self.most_recently_used();
                        for other in self.policy.foreground_demotions(&live, tab_id) {
                            if let Some(surface) = self.surfaces.get_mut(&other) {
                                surface.set_background(true);
                            }
                        }
                    }
                }
                Err(error) => {
                    notes.push(format!("compat surface {tab_id} failed: {error}"));
                }
            }
        }

        if !self.is_active() {
            self.environment = None;
            self.environment_receiver = None;
            self.environment_deadline = None;
        }

        notes
    }

    fn fresh_frame(&self, fallback: Frame) -> Frame {
        let Some((suffix, chrome)) = self.window_probe.as_ref() else {
            return fallback;
        };
        content_surface(suffix, *chrome).map_or(fallback, |surface| Frame {
            left: surface.left,
            top: surface.top,
            right: surface.right,
            bottom: surface.bottom,
        })
    }

    fn fail_environment(&mut self, reason: String) {
        self.environment_error = Some(reason);
        self.environment_receiver = None;
        self.environment_deadline = None;
        let pending: Vec<usize> = self.requests.keys().copied().collect();
        for tab_id in pending {
            self.abandon_request(tab_id);
        }
        self.surfaces.clear();
    }

    fn abandon_request(&mut self, tab_id: usize) {
        let mailbox = self.requests.remove(&tab_id).map(|request| request.mailbox);
        if let Some(mailbox) = mailbox {
            if let Some(controller) = abandon_mailbox(&mailbox) {
                let _ = unsafe { controller.Close() };
            }
        }
    }

    pub fn reload_surface(&mut self, tab_id: usize) -> Result<(), String> {
        let surface = self
            .surfaces
            .get_mut(&tab_id)
            .ok_or_else(|| "no webview surface for this tab".to_string())?;
        surface.last_used = Instant::now();
        surface.reload()
    }

    pub fn navigate_surface(&mut self, tab_id: usize, url: &str) -> Result<(), String> {
        let surface = self
            .surfaces
            .get_mut(&tab_id)
            .ok_or_else(|| "no webview surface for this tab".to_string())?;
        surface.last_used = Instant::now();
        surface.navigate(url)
    }

    pub fn set_surface_bounds(&mut self, tab_id: usize, frame: Frame) -> Result<(), String> {
        let surface = self
            .surfaces
            .get_mut(&tab_id)
            .ok_or_else(|| "no webview surface for this tab".to_string())?;
        surface.last_used = Instant::now();
        unsafe {
            surface
                .controller
                .SetBounds(frame.rect())
                .map_err(|error| error.to_string())
        }
    }

    pub fn close_surface(&mut self, tab_id: usize) -> bool {
        self.abandon_request(tab_id);
        if self.active_tab == Some(tab_id) {
            self.active_tab = None;
        }
        self.surfaces.remove(&tab_id).is_some()
    }

    pub fn apply_pressure(&mut self, report: CompatLoadReport) -> Vec<String> {
        let mut notes = Vec::new();
        let live = self.most_recently_used();
        let suspend_backgrounds = report.should_suspend_background(self.policy);
        let demotions = self
            .policy
            .foreground_demotions(&live, self.active_tab.unwrap_or(usize::MAX));
        for tab_id in live {
            let Some(surface) = self.surfaces.get_mut(&tab_id) else {
                continue;
            };
            if Some(tab_id) == self.active_tab {
                surface.set_foreground();
            } else {
                surface.set_background(suspend_backgrounds || demotions.contains(&tab_id));
            }
        }
        if report.over_ceiling() {
            let background: Vec<usize> = self
                .surfaces
                .keys()
                .copied()
                .filter(|tab_id| Some(*tab_id) != self.active_tab)
                .collect();
            for tab_id in background {
                self.surfaces.remove(&tab_id);
                notes.push(format!("compat surface {tab_id} recycled under pressure"));
            }
        } else {
            let exempt = self.active_tab.unwrap_or(usize::MAX);
            for tab_id in self.recycle_over_cap(exempt) {
                notes.push(format!(
                    "compat surface {tab_id} recycled beyond the live cap"
                ));
            }
        }
        notes
    }

    fn begin_environment(&mut self) -> Result<(), String> {
        if self.environment.is_some() || self.environment_receiver.is_some() {
            return Ok(());
        }
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok() {
            let reason = format!(
                "the compatibility backend needs a single-threaded apartment ({})",
                error.message()
            );
            self.fail_environment(reason.clone());
            return Err(reason);
        }
        let (sender, receiver) = channel();
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error_code, environment| {
                let resolved = if error_code.is_ok() {
                    environment
                } else {
                    None
                };
                let _ = sender.send(resolved);
                Ok(())
            },
        ));
        let started = match self.user_data_folder.as_ref() {
            Some(path) => {
                let encoded = CoTaskMemPWSTR::from(path.as_str());
                unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        windows_wv2::core::PCWSTR::null(),
                        *encoded.as_ref().as_pcwstr(),
                        None,
                        &handler,
                    )
                }
                .is_ok()
            }
            None => unsafe { CreateCoreWebView2Environment(&handler) }.is_ok(),
        };
        if !started {
            let reason = "the WebView2 runtime or its loader is unavailable".to_string();
            self.fail_environment(reason.clone());
            return Err(reason);
        }
        self.environment_receiver = Some(receiver);
        self.environment_deadline = Some(Instant::now() + ENVIRONMENT_TIMEOUT);
        Ok(())
    }

    fn begin_controller(&mut self, tab_id: usize) -> Result<(), String> {
        let Some(environment) = self.environment.clone() else {
            return Ok(());
        };
        let (parent_hwnd, incognito, mailbox) = match self.requests.get(&tab_id) {
            Some(request) => (
                request.parent_hwnd,
                request.incognito,
                Rc::clone(&request.mailbox),
            ),
            None => return Ok(()),
        };
        if create_is_outstanding(&mailbox) {
            return Ok(());
        }
        let slots = self
            .policy
            .max_concurrent
            .saturating_sub(self.surfaces.len())
            .saturating_sub(self.outstanding_creates());
        if slots == 0 {
            self.abandon_request(tab_id);
            return Err(format!(
                "compat controller for tab {tab_id} refused the live cap"
            ));
        }
        arm_create(&mailbox);
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |error_code, controller| {
                let resolved = if error_code.is_ok() { controller } else { None };
                if let Some(controller) = deliver_controller(&mailbox, resolved) {
                    let _ = unsafe { controller.Close() };
                }
                Ok(())
            },
        ));
        let parent = HWND(parent_hwnd as *mut c_void);
        let started = if incognito {
            create_inprivate_controller(&environment, parent, &handler)
        } else {
            unsafe { environment.CreateCoreWebView2Controller(parent, &handler) }.is_ok()
        };
        if !started {
            self.abandon_request(tab_id);
            return Err(format!(
                "compat controller for tab {tab_id} was refused by the WebView2 runtime"
            ));
        }
        Ok(())
    }
}

impl Drop for WebView2Host {
    fn drop(&mut self) {
        let pending: Vec<usize> = self.requests.keys().copied().collect();
        for tab_id in pending {
            self.abandon_request(tab_id);
        }
        self.surfaces.clear();
        self.environment = None;
    }
}

fn expired_requests(requests: &HashMap<usize, SurfaceRequest>, now: Instant) -> Vec<usize> {
    let mut expired: Vec<usize> = requests
        .iter()
        .filter(|(_, request)| now >= request.deadline)
        .map(|(tab_id, _)| *tab_id)
        .collect();
    expired.sort_unstable();
    expired
}

fn create_inprivate_controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
    handler: &ICoreWebView2CreateCoreWebView2ControllerCompletedHandler,
) -> bool {
    let Ok(environment10) = environment.cast::<ICoreWebView2Environment10>() else {
        return false;
    };
    let Ok(options) = (unsafe { environment10.CreateCoreWebView2ControllerOptions() }) else {
        return false;
    };
    let _ = unsafe { options.SetIsInPrivateModeEnabled(TRUE) };
    unsafe { environment10.CreateCoreWebView2ControllerWithOptions(parent, &options, handler) }
        .is_ok()
}

fn build_surface(
    guard: &mut ControllerGuard,
    frame: Frame,
    url: &str,
    environment: &ICoreWebView2Environment,
    blocker: &ResourceBlocker,
    visible: bool,
) -> Result<CompatTabSurface, String> {
    let controller = guard.reference()?;
    unsafe {
        controller
            .SetBounds(frame.rect())
            .map_err(|error| error.to_string())?;
        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                A: 255,
                R: 24,
                G: 24,
                B: 24,
            });
        }
    }
    let webview = unsafe {
        controller
            .CoreWebView2()
            .map_err(|error| error.to_string())?
    };
    let controller = guard
        .take()
        .ok_or_else(|| "compat controller is already consumed".to_string())?;
    let mut surface = CompatTabSurface {
        controller,
        webview,
        last_used: Instant::now(),
    };
    install_resource_blocking(&surface.webview, environment, url, blocker)?;
    if visible {
        surface.set_foreground();
    } else {
        surface.set_background(true);
    }
    surface.navigate(url)?;
    Ok(surface)
}

fn install_resource_blocking(
    webview: &ICoreWebView2,
    environment: &ICoreWebView2Environment,
    page_url: &str,
    blocker: &ResourceBlocker,
) -> Result<(), String> {
    let filter = CoTaskMemPWSTR::from("*");
    unsafe {
        webview
            .AddWebResourceRequestedFilter(
                *filter.as_ref().as_pcwstr(),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
            .map_err(|error| error.to_string())?;
    }
    let environment = environment.clone();
    let blocker = blocker.clone();
    let page = page_url.to_string();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let request = unsafe { args.Request() }?;
        let mut uri = PWSTR::null();
        unsafe { request.Uri(&mut uri) }?;
        let (target, terminated) = decode_pwstr(&uri);
        unsafe { CoTaskMemFree(Some(uri.0 as *const c_void)) };
        let blocked = !terminated || blocker(&target, &page, resource_hint(&target));
        if blocked {
            let response = unsafe {
                environment.CreateWebResourceResponse(None, 404, PCWSTR::null(), PCWSTR::null())
            }?;
            unsafe { args.SetResponse(&response) }?;
        }
        Ok(())
    }));
    let mut token = EventRegistrationToken::default();
    unsafe {
        webview
            .add_WebResourceRequested(&handler, &mut token)
            .map_err(|error| error.to_string())
    }
}

fn decode_pwstr(value: &PWSTR) -> (String, bool) {
    if value.0.is_null() {
        return (String::new(), true);
    }
    let mut units = Vec::new();
    let mut terminated = false;
    unsafe {
        for offset in 0..MAX_URI_UTF16_UNITS {
            let unit = *value.0.offset(offset);
            if unit == 0 {
                terminated = true;
                break;
            }
            units.push(unit);
        }
    }
    (String::from_utf16_lossy(&units), terminated)
}

#[derive(Debug, Clone, Copy)]
pub struct WindowSurface {
    pub parent_hwnd: isize,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

struct WindowScan {
    marker: String,
    found: isize,
    process_id: u32,
}

fn owns_window(hwnd: HWND, process_id: u32) -> bool {
    let mut window_process = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_process)) };
    window_process == process_id
}

unsafe extern "system" fn collect_sylphra_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    use windows_wv2::Win32::Foundation::{FALSE, TRUE};
    use windows_wv2::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    let scan = &mut *(lparam.0 as *mut WindowScan);
    if scan.found != 0 || !owns_window(hwnd, scan.process_id) || !IsWindowVisible(hwnd).as_bool() {
        return TRUE;
    }
    if window_title(hwnd).ends_with(scan.marker.as_str()) {
        scan.found = hwnd.0 as isize;
        return FALSE;
    }
    TRUE
}

pub fn surface_for_hwnd(hwnd: isize) -> Option<WindowSurface> {
    use windows_wv2::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindow};

    if hwnd == 0 {
        return None;
    }
    let window = HWND(hwnd as *mut c_void);
    if !unsafe { IsWindow(window) }.as_bool() {
        return None;
    }
    let mut client_rect = RECT::default();
    unsafe { GetClientRect(window, &mut client_rect) }.ok()?;
    Some(WindowSurface {
        parent_hwnd: hwnd,
        left: client_rect.left,
        top: client_rect.top,
        right: client_rect.right,
        bottom: client_rect.bottom,
    })
}

pub fn locate_window_surface(title_suffix: &str) -> Option<WindowSurface> {
    use windows_wv2::Win32::UI::WindowsAndMessaging::EnumWindows;

    let mut scan = WindowScan {
        marker: title_suffix.to_string(),
        found: 0,
        process_id: unsafe { GetCurrentProcessId() },
    };
    unsafe {
        let lparam = LPARAM(&mut scan as *mut WindowScan as isize);
        let _ = EnumWindows(Some(collect_sylphra_window), lparam);
    }
    surface_for_hwnd(scan.found)
}

pub fn chrome_inset(chrome_logical_height: i32, dpi: u32) -> i32 {
    let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
    (chrome_logical_height as f64 * scale).round() as i32
}

fn inset_surface(base: WindowSurface, chrome_logical_height: i32) -> Option<WindowSurface> {
    use windows_wv2::Win32::UI::HiDpi::GetDpiForWindow;

    let dpi = unsafe { GetDpiForWindow(HWND(base.parent_hwnd as *mut c_void)) };
    let inset = chrome_inset(chrome_logical_height, dpi);
    let top = base
        .top
        .saturating_add(inset)
        .min(base.bottom.saturating_sub(1));
    Some(WindowSurface {
        parent_hwnd: base.parent_hwnd,
        left: base.left,
        top,
        right: base.right,
        bottom: base.bottom,
    })
}

pub fn content_surface(title_suffix: &str, chrome_logical_height: i32) -> Option<WindowSurface> {
    inset_surface(locate_window_surface(title_suffix)?, chrome_logical_height)
}

pub fn content_surface_for_hwnd(hwnd: isize, chrome_logical_height: i32) -> Option<WindowSurface> {
    inset_surface(surface_for_hwnd(hwnd)?, chrome_logical_height)
}

fn window_title(hwnd: HWND) -> String {
    use windows_wv2::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW;
    use windows_wv2::Win32::UI::WindowsAndMessaging::GetWindowTextW;

    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; (length as usize) + 1];
    let written = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if written <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..written as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(deadline: Instant) -> SurfaceRequest {
        SurfaceRequest {
            parent_hwnd: 1,
            frame: Frame {
                left: 0,
                top: 0,
                right: 640,
                bottom: 480,
            },
            url: "https://example.test/feed".to_string(),
            incognito: false,
            blocker: Arc::new(|_, _, _| false),
            mailbox: new_mailbox(),
            deadline,
        }
    }

    #[test]
    fn expired_requests_are_ordered_and_respect_the_deadline() {
        let now = Instant::now();
        let mut requests = HashMap::new();
        requests.insert(7, request(now - Duration::from_millis(1)));
        requests.insert(3, request(now - Duration::from_millis(9)));
        requests.insert(9, request(now + REQUEST_TIMEOUT));
        assert_eq!(expired_requests(&requests, now), vec![3, 7]);
        assert!(expired_requests(&requests, now - Duration::from_secs(1)).is_empty());
        assert!(expired_requests(&HashMap::new(), now).is_empty());
    }

    #[test]
    fn hints_follow_the_path_extension() {
        assert_eq!(
            resource_hint("https://cdn.test/app.js?v=1"),
            ResourceHint::Script
        );
        assert_eq!(
            resource_hint("https://cdn.test/base.CSS"),
            ResourceHint::Style
        );
        assert_eq!(
            resource_hint("https://cdn.test/Banner.PNG#top"),
            ResourceHint::Image
        );
        assert_eq!(resource_hint("//cdn.test/f.woff2"), ResourceHint::Font);
        assert_eq!(
            resource_hint("https://cdn.test/clip.mp4"),
            ResourceHint::Media
        );
        assert_eq!(
            resource_hint("https://cdn.test/data.json"),
            ResourceHint::Fetch
        );
        assert_eq!(resource_hint("https://cdn.test/"), ResourceHint::Document);
        assert_eq!(resource_hint("https://cdn.test"), ResourceHint::Document);
        assert_eq!(
            resource_hint("https://cdn.test/segment"),
            ResourceHint::Other
        );
        assert_eq!(
            resource_hint("data:text/html;base64,AAAA"),
            ResourceHint::Other
        );
    }

    #[test]
    fn chrome_inset_scales_with_window_dpi() {
        assert_eq!(chrome_inset(110, 96), 110);
        assert_eq!(chrome_inset(110, 144), 165);
        assert_eq!(chrome_inset(110, 0), 110);
        assert_eq!(chrome_inset(0, 192), 0);
    }

    #[test]
    fn the_handoff_tracks_a_create_from_arm_to_take() {
        let mailbox = new_mailbox();
        assert!(!create_is_outstanding(&mailbox));
        arm_create(&mailbox);
        assert!(create_is_outstanding(&mailbox));
        assert!(take_controller(&mailbox).is_none());
        assert!(deliver_controller(&mailbox, None).is_none());
        assert!(create_is_outstanding(&mailbox));
        assert!(matches!(take_controller(&mailbox), Some(None)));
        assert!(!create_is_outstanding(&mailbox));
    }

    #[test]
    fn an_abandoned_handoff_refuses_a_late_delivery() {
        let mailbox = new_mailbox();
        arm_create(&mailbox);
        assert!(abandon_mailbox(&mailbox).is_none());
        assert!(!create_is_outstanding(&mailbox));
        assert!(take_controller(&mailbox).is_none());
        assert!(deliver_controller(&mailbox, None).is_none());
        assert!(!create_is_outstanding(&mailbox));
    }

    #[test]
    fn request_capacity_drops_the_oldest_pending_and_protects_the_newcomer() {
        let mut host = WebView2Host::new();
        let cap = host.policy().max_concurrent;
        let base = Instant::now();
        for index in 0..cap {
            host.requests
                .insert(index, request(base + Duration::from_millis(index as u64)));
        }
        host.requests
            .insert(cap, request(base + Duration::from_secs(999)));
        assert_eq!(host.pending_surface_count(), cap + 1);
        host.trim_requests_to_capacity(cap);
        assert_eq!(host.pending_surface_count(), cap);
        assert!(host.requests.contains_key(&cap));
        assert!(!host.requests.contains_key(&0));
    }

    #[test]
    fn expired_requests_protect_the_still_live_tab() {
        let mut host = WebView2Host::new();
        host.requests
            .insert(5, request(Instant::now() + REQUEST_TIMEOUT));
        host.requests
            .insert(6, request(Instant::now() + REQUEST_TIMEOUT));
        assert_eq!(host.pending_surface_count(), 2);
        assert!(host.is_tab_active(5));
        let expired = expired_requests(&host.requests, Instant::now() + REQUEST_TIMEOUT * 2);
        assert_eq!(expired, vec![5, 6]);
    }
}
