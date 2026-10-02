use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::child_process::{ChildProcessManager, ProcessState};
use crate::crash_recovery::CrashRecoveryEngine;
use crate::css_parser::CssRule;
use crate::document::PreparedDocument;
use crate::ipc::{split_text_chunks, IpcCommand, IpcReplyBody, RenderPart, MAX_IPC_CHUNK_BYTES};
use crate::memory_probe;
use crate::process_architecture::{ProcessId, ProcessRole, TabId};
use crate::resource_caps;
use crate::resource_ledger::{OwnerId, SharedLedger, SubsystemId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLossReason {
    Crashed,
    MemoryKilled,
    Shed,
}

#[derive(Debug, Clone)]
pub struct RendererLoss {
    pub origin: String,
    pub tabs: Vec<TabId>,
    pub reason: ProcessLossReason,
    pub reported_bytes: u64,
}

pub struct BrowserProcessCoordinator {
    program: PathBuf,
    pub manager: ChildProcessManager,
    pub recovery: CrashRecoveryEngine,
    service_processes: HashMap<ProcessId, ProcessRole>,
    renderers_by_origin: HashMap<String, ProcessId>,
    reported_memory: HashMap<ProcessId, u64>,
}

impl BrowserProcessCoordinator {
    pub fn discover() -> Result<Self, String> {
        let current = std::env::current_exe()
            .map_err(|error| format!("Cannot locate browser executable: {error}"))?;
        let program = current.with_file_name(format!(
            "sylphra-browser-child{}",
            std::env::consts::EXE_SUFFIX
        ));
        Self::start(program)
    }

    pub fn start(program: impl AsRef<Path>) -> Result<Self, String> {
        let program = program.as_ref().to_path_buf();
        if !program.is_file() {
            return Err(format!(
                "Native browser child is unavailable at {}",
                program.display()
            ));
        }
        let mut coordinator = Self {
            program,
            manager: ChildProcessManager::new(),
            recovery: CrashRecoveryEngine::new(),
            service_processes: HashMap::new(),
            renderers_by_origin: HashMap::new(),
            reported_memory: HashMap::new(),
        };
        for role in [ProcessRole::Network, ProcessRole::Media, ProcessRole::Gpu] {
            let process = coordinator
                .manager
                .spawn_native_process(role.clone(), &coordinator.program)?;
            let reply = coordinator
                .manager
                .send_native_command(process, IpcCommand::Heartbeat)?;
            if !reply.accepted {
                return Err(format!("{role} child rejected its startup heartbeat"));
            }
            coordinator.service_processes.insert(process, role);
        }
        Ok(coordinator)
    }

    pub fn attach_tab(&mut self, tab_id: usize, url: &str) -> Result<ProcessId, String> {
        let origin = site_origin(url)?;
        let process = match self.renderers_by_origin.get(&origin).copied() {
            Some(process)
                if self
                    .manager
                    .processes
                    .get(&process)
                    .is_some_and(|child| child.state == ProcessState::Running) =>
            {
                process
            }
            _ => {
                let process = self.manager.spawn_native_process(
                    ProcessRole::Renderer {
                        origin: origin.clone(),
                    },
                    &self.program,
                )?;
                self.renderers_by_origin.insert(origin.clone(), process);
                process
            }
        };
        self.recovery
            .register_tab(TabId(tab_id as u64), url.to_string(), process);
        let reply = self.manager.send_native_command(
            process,
            IpcCommand::Navigate {
                url: url.to_string(),
            },
        )?;
        if !reply.accepted {
            return Err("Renderer process rejected navigation".to_string());
        }
        Ok(process)
    }

    pub fn heartbeat_and_recover(&mut self) -> Vec<String> {
        let process_ids: Vec<ProcessId> = self.manager.processes.keys().copied().collect();
        let mut failures = Vec::new();
        for process in process_ids {
            if self
                .manager
                .processes
                .get(&process)
                .is_some_and(|child| child.state != ProcessState::Running)
            {
                continue;
            }
            if let Err(error) = self
                .manager
                .send_native_command(process, IpcCommand::Heartbeat)
            {
                failures.push(format!("{process:?}: {error}"));
                self.recovery.inject_fault_crash(&mut self.manager, process);
                if let Ok(replacement) = self.recovery.recover_process(&mut self.manager, process) {
                    for renderer in self.renderers_by_origin.values_mut() {
                        if *renderer == process {
                            *renderer = replacement;
                        }
                    }
                    if let Some(role) = self.service_processes.remove(&process) {
                        self.service_processes.insert(replacement, role);
                    }
                }
            }
        }
        failures
    }

    pub fn native_process_count(&self) -> usize {
        self.manager
            .processes
            .keys()
            .filter(|process| self.manager.native_os_id(**process).is_some())
            .count()
    }

    pub fn renderer_origins(&self) -> Vec<(String, ProcessId)> {
        self.renderers_by_origin.clone().into_iter().collect()
    }

    pub fn tabs_for_process(&self, process: ProcessId) -> Vec<TabId> {
        self.recovery
            .tabs
            .values()
            .filter(|tab| tab.process_id == process)
            .map(|tab| tab.tab_id)
            .collect()
    }

    pub fn origin_of_process(&self, process: ProcessId) -> Option<String> {
        self.renderers_by_origin
            .iter()
            .find(|(_, assigned)| **assigned == process)
            .map(|(origin, _)| origin.clone())
    }

    fn note_reply_memory(
        &mut self,
        ledger: &SharedLedger,
        process: ProcessId,
        body: &IpcReplyBody,
    ) {
        let reported = match body {
            IpcReplyBody::Memory { working_set_bytes } => *working_set_bytes,
            _ => self
                .manager
                .native_os_id(process)
                .and_then(memory_probe::process_working_set_by_os_id)
                .unwrap_or(0),
        };
        if reported == 0 {
            return;
        }
        self.reported_memory.insert(process, reported);
        if let Some(os_id) = self.manager.native_os_id(process) {
            ledger.note_child_working_set(os_id, reported);
        }
    }

    fn forget_process_memory(&mut self, ledger: &SharedLedger, process: ProcessId) {
        if let Some(os_id) = self.manager.native_os_id(process) {
            ledger.note_child_working_set(os_id, 0);
        }
        self.reported_memory.remove(&process);
    }

    fn kill_and_recover(
        &mut self,
        ledger: &SharedLedger,
        process: ProcessId,
        reason: ProcessLossReason,
    ) -> Option<RendererLoss> {
        let origin = self.origin_of_process(process);
        let tabs = self.tabs_for_process(process);
        self.forget_process_memory(ledger, process);
        self.recovery.inject_fault_crash(&mut self.manager, process);
        if let Ok(replacement) = self.recovery.recover_process(&mut self.manager, process) {
            for renderer in self.renderers_by_origin.values_mut() {
                if *renderer == process {
                    *renderer = replacement;
                }
            }
            if let Some(role) = self.service_processes.remove(&process) {
                self.service_processes.insert(replacement, role);
            }
        }
        Some(RendererLoss {
            origin: origin.unwrap_or_default(),
            tabs,
            reason,
            reported_bytes: 0,
        })
    }

    pub fn poll_process_memory(&mut self, ledger: &SharedLedger) -> Vec<RendererLoss> {
        let process_ids: Vec<ProcessId> = self.manager.processes.keys().copied().collect();
        let mut losses = Vec::new();
        for process in process_ids {
            let running = self
                .manager
                .processes
                .get(&process)
                .is_some_and(|child| child.state == ProcessState::Running);
            if !running {
                losses.extend(self.kill_and_recover(ledger, process, ProcessLossReason::Crashed));
                continue;
            }
            match self.manager.send_native_command(
                process,
                IpcCommand::MemoryReport {
                    working_set_bytes: 0,
                },
            ) {
                Ok(reply) => self.note_reply_memory(ledger, process, &reply.body),
                Err(_) => {
                    losses.extend(self.kill_and_recover(
                        ledger,
                        process,
                        ProcessLossReason::Crashed,
                    ));
                    continue;
                }
            }
            let limit = self
                .manager
                .processes
                .get(&process)
                .map(|child| child.metadata.role.memory_limit_bytes())
                .unwrap_or(0);
            let reported = self.reported_memory.get(&process).copied().unwrap_or(0);
            if memory_limit_exceeded(limit, reported) {
                let mut loss =
                    self.kill_and_recover(ledger, process, ProcessLossReason::MemoryKilled);
                if let Some(loss) = loss.as_mut() {
                    loss.reported_bytes = reported;
                }
                losses.extend(loss);
            }
        }
        losses
    }

    fn command(
        &mut self,
        ledger: &SharedLedger,
        process: ProcessId,
        command: IpcCommand,
    ) -> Result<IpcReplyBody, String> {
        let reply = self.manager.send_native_command(process, command)?;
        self.note_reply_memory(ledger, process, &reply.body);
        if !reply.accepted {
            return Err("renderer rejected a command: ".to_string() + &reply.detail);
        }
        Ok(reply.body)
    }

    pub fn shed_origin(&mut self, ledger: &SharedLedger, origin: &str) -> Option<RendererLoss> {
        let process = self.renderers_by_origin.remove(origin)?;
        let tabs = self.tabs_for_process(process);
        let reported_bytes = self.reported_memory.get(&process).copied().unwrap_or(0);
        self.forget_process_memory(ledger, process);
        self.manager.terminate_process(process);
        Some(RendererLoss {
            origin: origin.to_string(),
            tabs,
            reason: ProcessLossReason::Shed,
            reported_bytes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_document(
        &mut self,
        ledger: &SharedLedger,
        tab_id: usize,
        url: &str,
        html: &str,
        base_rules: &[CssRule],
        viewport_width: u32,
        viewport_height: u32,
    ) -> Result<PreparedDocument, String> {
        let process = self.attach_tab(tab_id, url)?;
        let generation = self
            .manager
            .processes
            .get(&process)
            .ok_or_else(|| "renderer vanished before rendering".to_string())?
            .metadata
            .generation;
        let styles = serde_json::to_string(base_rules).map_err(|error| error.to_string())?;
        let declared = html.len().saturating_add(styles.len());
        let total_budget = resource_caps::DOM_TRANSFER_TOTAL_BYTES;
        if declared > total_budget {
            return Err("document exceeds the renderer transfer budget".to_string());
        }
        let lease = ledger
            .lease(SubsystemId::WorkerIpc, OwnerId::Tab(tab_id), declared)
            .map_err(|denied| denied.message())?;
        self.command(
            ledger,
            process,
            IpcCommand::RenderBegin {
                tab_id: tab_id as u64,
                generation: generation.0,
                style_bytes: styles.len() as u64,
                html_bytes: html.len() as u64,
                viewport_width,
                viewport_height,
            },
        )?;
        let mut sequence = 0_u64;
        for chunk in split_text_chunks(&styles, MAX_IPC_CHUNK_BYTES) {
            self.upload(ledger, process, sequence, RenderPart::Styles, chunk)?;
            sequence += 1;
        }
        for chunk in split_text_chunks(html, MAX_IPC_CHUNK_BYTES) {
            self.upload(ledger, process, sequence, RenderPart::Html, chunk)?;
            sequence += 1;
        }
        let announcement =
            self.command(ledger, process, IpcCommand::RenderEnd { seq: sequence })?;
        let (node_count, total_bytes) = match announcement {
            IpcReplyBody::DomBegin {
                tab_id: echoed,
                node_count,
                total_bytes,
            } => {
                if echoed != tab_id as u64 {
                    return Err("renderer answered for another tab".to_string());
                }
                (node_count, total_bytes)
            }
            other => {
                drop(lease);
                return Err("renderer did not open a document transfer: ".to_string()
                    + &format!("{other:?}"));
            }
        };
        if total_bytes as usize > total_budget || node_count as usize > resource_caps::DOM_MAX_NODES
        {
            return Err("rendered document exceeds its transfer budget".to_string());
        }
        let mut payload = String::new();
        let mut next = 0_u64;
        loop {
            let request = self.command(
                ledger,
                process,
                IpcCommand::DomRequest {
                    tab_id: tab_id as u64,
                    seq: next,
                },
            )?;
            match request {
                IpcReplyBody::DomChunk { seq, text } => {
                    if seq != next {
                        return Err("document chunks arrived out of order".to_string());
                    }
                    if payload.len().saturating_add(text.len()) > total_bytes as usize {
                        return Err("document exceeded its declared size".to_string());
                    }
                    payload.push_str(&text);
                    next += 1;
                }
                IpcReplyBody::DomEnd { .. } => break,
                other => {
                    drop(lease);
                    return Err("renderer aborted the document transfer: ".to_string()
                        + &format!("{other:?}"));
                }
            }
        }
        drop(lease);
        if payload.len() != total_bytes as usize {
            return Err("document length does not match its declaration".to_string());
        }
        serde_json::from_str(&payload).map_err(|error| error.to_string())
    }

    fn upload(
        &mut self,
        ledger: &SharedLedger,
        process: ProcessId,
        seq: u64,
        part: RenderPart,
        text: String,
    ) -> Result<(), String> {
        self.command(ledger, process, IpcCommand::RenderChunk { seq, part, text })?;
        Ok(())
    }
}

pub fn memory_limit_exceeded(limit_bytes: usize, reported_bytes: u64) -> bool {
    let Ok(limit) = usize::try_from(reported_bytes) else {
        return limit_bytes > 0;
    };
    limit_bytes > 0 && limit >= limit_bytes
}

fn site_origin(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "Cannot isolate an invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Only HTTP(S) pages receive renderer processes".to_string());
    }
    Ok(parsed.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_origin_is_partitioned() {
        assert_eq!(
            site_origin("https://example.test:8443/a").unwrap(),
            "https://example.test:8443"
        );
        assert!(site_origin("file:///tmp/a.html").is_err());
    }
}
