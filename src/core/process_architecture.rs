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

impl ProcessRole {
    pub fn memory_limit_bytes(&self) -> usize {
        match self {
            ProcessRole::Browser => crate::resource_caps::BROWSER_PROCESS_MEMORY_BYTES,
            ProcessRole::Renderer { .. } => crate::resource_caps::RENDERER_PROCESS_MEMORY_BYTES,
            ProcessRole::Network => crate::resource_caps::NETWORK_PROCESS_MEMORY_BYTES,
            ProcessRole::Media => crate::resource_caps::MEDIA_PROCESS_MEMORY_BYTES,
            ProcessRole::Gpu => crate::resource_caps::GPU_PROCESS_MEMORY_BYTES,
        }
    }

    pub fn enforces_ui_restrictions(&self) -> bool {
        !matches!(self, ProcessRole::Browser | ProcessRole::Gpu)
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
        let memory_limit_bytes = role.memory_limit_bytes();

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
        assert_eq!(
            meta.memory_limit_bytes,
            crate::resource_caps::RENDERER_PROCESS_MEMORY_BYTES
        );
        assert!(
            meta.memory_limit_bytes
                < crate::resource_caps::mb(crate::resource_caps::GLOBAL_HARD_LIMIT_MB)
        );
        assert!(ProcessRole::Browser.memory_limit_bytes() > meta.memory_limit_bytes);
        assert!(ProcessRole::Renderer {
            origin: String::new()
        }
        .enforces_ui_restrictions());
        assert!(!ProcessRole::Browser.enforces_ui_restrictions());
    }
}
