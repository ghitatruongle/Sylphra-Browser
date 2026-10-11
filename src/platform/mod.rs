pub mod app_platform;
pub mod child_process;
pub mod compat_backend;
pub mod compat_governance;
pub mod compat_session;
pub mod installed_app;
pub mod ipc;
pub mod process_coordinator;
pub mod release_metrics;
pub mod release_smoke;
pub mod sandbox;
pub mod updater;
#[cfg(all(windows, feature = "compat-webview2"))]
pub mod webview2_host;
pub mod windows_integration;
