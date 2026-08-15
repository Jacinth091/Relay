use std::path::PathBuf;

/// Provider-neutral execution activity events for TUI consumption.
/// Provider adapters translate their native events into these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeActivity {
    /// Session has started with the provider.
    SessionStarted,
    /// Agent is analyzing/thinking (no specific command).
    Analyzing { description: String },
    /// A command has started executing.
    CommandStarted {
        /// Logical command (e.g. "cargo test --workspace"), not raw shell wrapper
        logical_command: String,
        /// Full raw command for logs (may include shell wrappers)
        raw_command: Option<String>,
    },
    /// Intermediate command output/progress.
    CommandProgress {
        logical_command: String,
        progress_line: String,
    },
    /// A command has completed.
    CommandCompleted {
        logical_command: String,
        success: bool,
    },
    /// A file has been changed by the agent.
    FileChanged {
        path: PathBuf,
        change_kind: FileChangeKind,
    },
    /// Verification step started.
    VerificationStarted { description: String },
    /// Verification step progress.
    VerificationProgress { description: String, detail: String },
    /// Verification step completed.
    VerificationCompleted { description: String, passed: bool },
    /// Agent produced a human-readable message.
    AgentMessage { message: String },
    /// Non-critical warning from the provider.
    Warning { message: String },
    /// Provider-specific diagnostic (for logs, not primary UX).
    ProviderDiagnostic { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

impl FileChangeKind {
    pub fn marker(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
        }
    }
}
