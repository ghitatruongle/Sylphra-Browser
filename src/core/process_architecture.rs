use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProcessRole {
    Browser,
    Renderer { origin: String },
    Network,
    Media,
    Gpu,
}

impl fmt::Display for ProcessRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessRole::Browser => write!(f, "Browser"),
            ProcessRole::Renderer { origin } => write!(f, "Renderer[{origin}]"),
            ProcessRole::Network => write!(f, "Network"),
            ProcessRole::Media => write!(f, "Media"),
            ProcessRole::Gpu => write!(f, "Gpu"),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ProcessId(pub u64);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct TabId(pub u64);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct GenerationId(pub u64);

impl GenerationId {
    pub fn next(&self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub id: ProcessId,
    pub role: ProcessRole,
    pub generation: GenerationId,
    pub alive: bool,
    pub memory_limit_bytes: usize,
}

impl ProcessMetadata {
    pub fn new(id: ProcessId, role: ProcessRole, generation: GenerationId) -> Self {
        let memory_limit_bytes = match &role {
            ProcessRole::Renderer { .. } => 512 * 1024 * 1024,
            ProcessRole::Network => 256 * 1024 * 1024,
            ProcessRole::Media => 512 * 1024 * 1024,
            ProcessRole::Gpu => 1024 * 1024 * 1024,
            ProcessRole::Browser => 2048 * 1024 * 1024,
        };

        Self {
            id,
            role,
            generation,
            alive: true,
            memory_limit_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_role_formatting_and_memory_limits() {
        let role = ProcessRole::Renderer {
            origin: "https://example.com".to_string(),
        };
        assert_eq!(role.to_string(), "Renderer[https://example.com]");

        let meta = ProcessMetadata::new(ProcessId(1), role, GenerationId(1));
        assert_eq!(meta.memory_limit_bytes, 512 * 1024 * 1024);
    }
}
