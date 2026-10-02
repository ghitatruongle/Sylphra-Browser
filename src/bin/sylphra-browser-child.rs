use std::collections::VecDeque;
use std::io::{BufRead, Write};

use sylphra::css_parser::CssRule;
use sylphra::ipc::{
    split_text_chunks, IpcCommand, IpcMessage, IpcReplyBody, NativeIpcReply, RenderPart,
    IPC_VERSION, MAX_IPC_CHUNK_BYTES, MAX_IPC_MESSAGE_BYTES,
};
use sylphra::process_architecture::{GenerationId, ProcessId, ProcessRole};
use sylphra::resource_caps;

const STAGE_CAP_BYTES: usize = resource_caps::DOM_TRANSFER_TOTAL_BYTES;
const SKELETON_TEXT_BUDGET: usize = resource_caps::mb(4);
const MEMORY_STRESS_CAP_BYTES: usize = resource_caps::mb(512);

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

#[derive(Default)]
struct Renderer {
    tab_id: u64,
    title: String,
    styles: String,
    html: String,
    style_bytes: u64,
    html_bytes: u64,
    viewport: (u32, u32),
    sequence: u64,
    chunks: VecDeque<String>,
    next_chunk: u64,
    node_count: u64,
    total_bytes: u64,
}

impl Renderer {
    fn begin(&mut self, tab_id: u64, style_bytes: u64, html_bytes: u64, viewport: (u32, u32)) {
        self.tab_id = tab_id;
        self.styles.clear();
        self.html.clear();
        self.style_bytes = style_bytes;
        self.html_bytes = html_bytes;
        self.viewport = viewport;
        self.sequence = 0;
        self.chunks.clear();
        self.next_chunk = 0;
        self.node_count = 0;
        self.total_bytes = 0;
    }

    fn reset_output(&mut self) {
        self.chunks.clear();
        self.next_chunk = 0;
        self.node_count = 0;
        self.total_bytes = 0;
    }
}

fn rejected(reason: impl Into<String>) -> (bool, String, IpcReplyBody) {
    let reason = reason.into();
    (false, reason.clone(), IpcReplyBody::Rejected { reason })
}

fn self_working_set() -> u64 {
    sylphra::memory_probe::current_process_working_set_bytes().unwrap_or_default()
}

fn handle(
    renderer: &mut Renderer,
    message: &IpcMessage,
    role: &ProcessRole,
) -> (bool, String, IpcReplyBody) {
    match &message.command {
        IpcCommand::Shutdown => (true, "shutdown".to_string(), IpcReplyBody::Idle),
        IpcCommand::Heartbeat => (
            true,
            format!("{role}: heartbeat"),
            IpcReplyBody::Memory {
                working_set_bytes: self_working_set(),
            },
        ),
        IpcCommand::MemoryReport { .. } => (
            true,
            format!("{role}: memory report"),
            IpcReplyBody::Memory {
                working_set_bytes: self_working_set(),
            },
        ),
        IpcCommand::MemoryStress { bytes } => {
            if !cfg!(feature = "renderer-memory-harness") {
                return rejected("memory stress is unavailable in this build");
            }
            let Ok(requested) = usize::try_from(*bytes) else {
                return rejected("memory stress request does not fit this address space");
            };
            if requested > MEMORY_STRESS_CAP_BYTES {
                return rejected("memory stress request exceeds the harness budget");
            }
            let mut block: Vec<u8> = Vec::new();
            if block.try_reserve_exact(requested).is_err() {
                return rejected("memory stress allocation was denied");
            }
            block.resize(requested, 7);
            std::mem::forget(block);
            (
                true,
                format!("{role}: holding {requested} bytes"),
                IpcReplyBody::Memory {
                    working_set_bytes: self_working_set(),
                },
            )
        }
        IpcCommand::Navigate { url } => {
            renderer.title = url.clone();
            renderer.reset_output();
            (
                true,
                format!("{role}: accepted navigation"),
                IpcReplyBody::Idle,
            )
        }
        IpcCommand::RenderBegin {
            tab_id,
            generation,
            style_bytes,
            html_bytes,
            viewport_width,
            viewport_height,
        } => {
            if !matches!(role, ProcessRole::Renderer { .. }) {
                return rejected("this process is not a renderer");
            }
            if *generation != message.target_generation.0 {
                return rejected("render generation does not match this process");
            }
            if *style_bytes as usize > STAGE_CAP_BYTES || *html_bytes as usize > STAGE_CAP_BYTES {
                return rejected("document exceeds the render stage budget");
            }
            renderer.begin(
                *tab_id,
                *style_bytes,
                *html_bytes,
                (*viewport_width, *viewport_height),
            );
            (
                true,
                format!("render stage opened for tab {tab_id}"),
                IpcReplyBody::DomAck { next_seq: 0 },
            )
        }
        IpcCommand::RenderChunk { seq, part, text } => {
            if *seq != renderer.sequence {
                return rejected(format!("unexpected chunk sequence {seq}"));
            }
            if text.len() > MAX_IPC_CHUNK_BYTES {
                return rejected("chunk exceeds the frame budget");
            }
            match part {
                RenderPart::Styles => renderer.styles.push_str(text),
                RenderPart::Html => renderer.html.push_str(text),
            }
            let (staged, declared) = match part {
                RenderPart::Styles => (renderer.styles.len(), renderer.style_bytes as usize),
                RenderPart::Html => (renderer.html.len(), renderer.html_bytes as usize),
            };
            if staged > declared {
                return rejected("chunk stream exceeded its declared byte budget");
            }
            renderer.sequence += 1;
            renderer.reset_output();
            (
                true,
                format!("chunk {seq} staged"),
                IpcReplyBody::DomAck {
                    next_seq: renderer.sequence,
                },
            )
        }
        IpcCommand::RenderEnd { seq } => {
            if *seq != renderer.sequence {
                return rejected(format!("unexpected end sequence {seq}"));
            }
            if renderer.html.len() != renderer.html_bytes as usize
                || renderer.styles.len() != renderer.style_bytes as usize
            {
                return rejected("chunk stream length does not match its declaration");
            }
            let styles = std::mem::take(&mut renderer.styles);
            let rules: Vec<CssRule> = serde_json::from_str(&styles).unwrap_or_default();
            drop(styles);
            let html = std::mem::take(&mut renderer.html);
            let mut prepared = sylphra::document::prepare_document(
                &html,
                &renderer.title,
                &rules,
                renderer.viewport.0,
                renderer.viewport.1,
            );
            drop(html);
            drop(rules);
            let mut payload = match serde_json::to_string(&prepared) {
                Ok(payload) => payload,
                Err(error) => return rejected(format!("cannot serialise document: {error}")),
            };
            if payload.len() > STAGE_CAP_BYTES {
                sylphra::document::skeleton_document(
                    &mut prepared,
                    resource_caps::DOM_TRANSFER_SKELETON_DEPTH,
                    SKELETON_TEXT_BUDGET,
                );
                payload = match serde_json::to_string(&prepared) {
                    Ok(payload) => payload,
                    Err(error) => return rejected(format!("cannot serialise skeleton: {error}")),
                };
            }
            renderer.node_count = prepared.stats.dom_nodes as u64;
            renderer.total_bytes = payload.len() as u64;
            renderer.chunks = VecDeque::from(split_text_chunks(&payload, MAX_IPC_CHUNK_BYTES));
            renderer.next_chunk = 0;
            drop(payload);
            (
                true,
                format!("document rendered in {} bytes", renderer.total_bytes),
                IpcReplyBody::DomBegin {
                    tab_id: renderer.tab_id,
                    node_count: renderer.node_count,
                    total_bytes: renderer.total_bytes,
                },
            )
        }
        IpcCommand::DomRequest { tab_id, seq } => {
            if *tab_id != renderer.tab_id || renderer.total_bytes == 0 {
                return rejected("no rendered document is staged for this tab");
            }
            if *seq != renderer.next_chunk {
                return rejected(format!("unexpected document request sequence {seq}"));
            }
            let index = renderer.next_chunk;
            if let Some(text) = renderer.chunks.pop_front() {
                renderer.next_chunk += 1;
                (
                    true,
                    format!("document chunk {index}"),
                    IpcReplyBody::DomChunk { seq: index, text },
                )
            } else {
                (
                    true,
                    "document transfer complete".to_string(),
                    IpcReplyBody::DomEnd { seq: index },
                )
            }
        }
        IpcCommand::DomBegin { .. } | IpcCommand::DomChunk { .. } | IpcCommand::DomEnd { .. } => {
            rejected("document transfer is initiated by the renderer")
        }
        IpcCommand::RenderFrame { .. }
        | IpcCommand::FetchResource { .. }
        | IpcCommand::PlayMedia { .. } => (
            true,
            format!("{role}: accepted command"),
            IpcReplyBody::Idle,
        ),
    }
}

fn run() -> Result<(), String> {
    sylphra::worker::apply_restricted_worker_token().map_err(|error| error.to_string())?;
    let mut arguments = std::env::args().skip(1);
    let process_id = arguments
        .next()
        .ok_or_else(|| "missing process id".to_string())?
        .parse::<u64>()
        .map(ProcessId)
        .map_err(|_| "invalid process id".to_string())?;
    let generation = arguments
        .next()
        .ok_or_else(|| "missing generation".to_string())?
        .parse::<u64>()
        .map(GenerationId)
        .map_err(|_| "invalid generation".to_string())?;
    let role: ProcessRole = serde_json::from_str(
        &arguments
            .next()
            .ok_or_else(|| "missing process role".to_string())?,
    )
    .map_err(|error| format!("invalid process role: {error}"))?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut renderer = Renderer::default();
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC request exceeds its byte budget".to_string());
        }
        let message: IpcMessage =
            serde_json::from_str(&line).map_err(|error| format!("invalid IPC request: {error}"))?;
        let accepted = message.version == IPC_VERSION
            && message.target_id == process_id
            && message.target_generation == generation;
        let (handled, detail, body) = if accepted {
            handle(&mut renderer, &message, &role)
        } else {
            (
                false,
                "rejected version, process id or generation".to_string(),
                IpcReplyBody::Rejected {
                    reason: "message identity does not match this process".to_string(),
                },
            )
        };
        let should_shutdown = detail == "shutdown";
        let reply = NativeIpcReply {
            version: IPC_VERSION,
            process_id,
            generation,
            sequence: message.sequence,
            accepted: accepted && handled,
            detail,
            body,
        };
        let encoded = serde_json::to_vec(&reply).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC response exceeds its byte budget".to_string());
        }
        stdout
            .write_all(&encoded)
            .and_then(|_| stdout.write_all(b"\n"))
            .and_then(|_| stdout.flush())
            .map_err(|error| error.to_string())?;
        if should_shutdown {
            break;
        }
    }
    Ok(())
}
