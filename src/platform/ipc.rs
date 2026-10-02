use crate::process_architecture::{GenerationId, ProcessId};

pub const IPC_VERSION: u32 = 1;
pub const MAX_IPC_MESSAGE_BYTES: usize = crate::resource_caps::IPC_MESSAGE_BYTES;
pub const MAX_IPC_CHUNK_BYTES: usize = crate::resource_caps::IPC_CHUNK_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RenderPart {
    Styles,
    Html,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IpcCommand {
    Navigate {
        url: String,
    },
    RenderFrame {
        html: String,
    },
    FetchResource {
        url: String,
        method: String,
    },
    PlayMedia {
        src: String,
    },
    Heartbeat,
    Shutdown,
    MemoryStress {
        bytes: u64,
    },
    RenderBegin {
        tab_id: u64,
        generation: u64,
        style_bytes: u64,
        html_bytes: u64,
        viewport_width: u32,
        viewport_height: u32,
    },
    RenderChunk {
        seq: u64,
        part: RenderPart,
        text: String,
    },
    RenderEnd {
        seq: u64,
    },
    DomBegin {
        tab_id: u64,
        generation: u64,
        node_count: u64,
        total_bytes: u64,
    },
    DomChunk {
        seq: u64,
        text: String,
    },
    DomEnd {
        seq: u64,
    },
    DomRequest {
        tab_id: u64,
        seq: u64,
    },
    MemoryReport {
        working_set_bytes: u64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IpcReplyBody {
    #[default]
    Idle,
    DomAck {
        next_seq: u64,
    },
    DomBegin {
        tab_id: u64,
        node_count: u64,
        total_bytes: u64,
    },
    DomChunk {
        seq: u64,
        text: String,
    },
    DomEnd {
        seq: u64,
    },
    Memory {
        working_set_bytes: u64,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IpcMessage {
    pub version: u32,
    pub sender_id: ProcessId,
    pub sender_generation: GenerationId,
    pub target_id: ProcessId,
    pub target_generation: GenerationId,
    pub sequence: u64,
    pub command: IpcCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeIpcReply {
    pub version: u32,
    pub process_id: ProcessId,
    pub generation: GenerationId,
    pub sequence: u64,
    pub accepted: bool,
    pub detail: String,
    #[serde(default)]
    pub body: IpcReplyBody,
}

pub fn split_text_chunks(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 || text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut current = 0usize;
    for (offset, ch) in text.char_indices() {
        let width = ch.len_utf8();
        if current + width > limit && current > 0 {
            chunks.push(text[start..offset].to_string());
            start = offset;
            current = width;
        } else {
            current += width;
        }
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    chunks
}

pub struct IpcChannel {
    pub channel_id: u64,
    pub sender_id: ProcessId,
    pub sender_generation: GenerationId,
    pub target_id: ProcessId,
    pub target_generation: GenerationId,
    pub sequence: u64,
    pub send_queue: Vec<IpcMessage>,
    pub receive_queue: Vec<IpcMessage>,
    pub closed: bool,
    last_received_sequence: u64,
}

impl IpcChannel {
    pub fn new(
        channel_id: u64,
        sender_id: ProcessId,
        sender_generation: GenerationId,
        target_id: ProcessId,
        target_generation: GenerationId,
    ) -> Self {
        Self {
            channel_id,
            sender_id,
            sender_generation,
            target_id,
            target_generation,
            sequence: 1,
            send_queue: Vec::new(),
            receive_queue: Vec::new(),
            closed: false,
            last_received_sequence: 0,
        }
    }

    pub fn send(&mut self, command: IpcCommand) -> Result<(), String> {
        if self.closed {
            return Err("IPC channel is closed".to_string());
        }

        let seq = self.sequence;
        self.sequence += 1;

        let message = IpcMessage {
            version: IPC_VERSION,
            sender_id: self.sender_id,
            sender_generation: self.sender_generation,
            target_id: self.target_id,
            target_generation: self.target_generation,
            sequence: seq,
            command,
        };

        let encoded = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_IPC_MESSAGE_BYTES {
            return Err("IPC message exceeds its byte budget".to_string());
        }
        if self.send_queue.len() >= 1000 {
            return Err("IPC send queue budget exceeded".to_string());
        }

        self.send_queue.push(message);
        Ok(())
    }

    pub fn deliver_incoming(&mut self, message: IpcMessage) -> Result<(), String> {
        if self.closed {
            return Err("IPC channel is closed".to_string());
        }

        if message.version != IPC_VERSION {
            return Err(format!(
                "IPC version mismatch: expected {}, got {}",
                IPC_VERSION, message.version
            ));
        }

        if message.target_generation != self.target_generation {
            return Err("StaleGenerationMessageDropped".to_string());
        }
        if message.target_id != self.target_id
            || message.sender_id != self.sender_id
            || message.sender_generation != self.sender_generation
        {
            return Err("IPC endpoint identity mismatch".to_string());
        }
        if message.sequence <= self.last_received_sequence {
            return Err("IPC replay or out-of-order sequence rejected".to_string());
        }

        if self.receive_queue.len() >= 1000 {
            return Err("IPC receive queue budget exceeded".to_string());
        }

        self.last_received_sequence = message.sequence;
        self.receive_queue.push(message);
        Ok(())
    }

    pub fn receive(&mut self) -> Option<IpcMessage> {
        if self.closed || self.receive_queue.is_empty() {
            None
        } else {
            Some(self.receive_queue.remove(0))
        }
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.send_queue.clear();
        self.receive_queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_never_split_a_multibyte_character() {
        let source = "漢".repeat(400);
        let chunks = split_text_chunks(&source, 1_000);
        assert!(chunks.len() > 1);
        let rejoined = chunks.concat();
        assert_eq!(rejoined, source);
        for chunk in &chunks {
            assert!(chunk.len() <= 1_000);
        }
        assert!(split_text_chunks("", 10).is_empty());
        assert!(split_text_chunks("abc", 0).is_empty());
    }

    #[test]
    fn ipc_channel_version_and_generation_checking() {
        let mut ch = IpcChannel::new(
            1,
            ProcessId(1),
            GenerationId(1),
            ProcessId(2),
            GenerationId(1),
        );

        ch.send(IpcCommand::Navigate {
            url: "https://example.com".to_string(),
        })
        .unwrap();

        let msg = ch.send_queue.remove(0);
        ch.deliver_incoming(msg).unwrap();

        let received = ch.receive().unwrap();
        assert_eq!(
            received.command,
            IpcCommand::Navigate {
                url: "https://example.com".to_string()
            }
        );

        let stale_msg = IpcMessage {
            version: IPC_VERSION,
            sender_id: ProcessId(1),
            sender_generation: GenerationId(1),
            target_id: ProcessId(2),
            target_generation: GenerationId(0),
            sequence: 99,
            command: IpcCommand::Heartbeat,
        };

        let err = ch.deliver_incoming(stale_msg).unwrap_err();
        assert_eq!(err, "StaleGenerationMessageDropped");
    }
}
