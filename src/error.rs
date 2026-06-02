use std::process::ExitCode;

/// Errors that cause edict to exit with a specific code.
#[derive(Debug, thiserror::Error)]
pub enum ExitError {
    #[error("config error: {0}")]
    Config(String),

    #[error("tool not found: {tool}")]
    ToolNotFound { tool: String },

    #[error("{tool} failed (exit {code}): {message}")]
    ToolFailed {
        tool: String,
        code: i32,
        message: String,
    },

    #[error("{tool} timed out after {timeout_secs}s")]
    Timeout { tool: String, timeout_secs: u64 },

    #[error("{message}")]
    WithCode { code: u8, message: String },

    #[error("audit failed")]
    AuditFailed,

    #[error("{0}")]
    Other(String),
}

impl ExitError {
    #[must_use] 
    pub const fn new(code: u8, message: String) -> Self {
        Self::WithCode { code, message }
    }

    #[must_use] 
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Config(_) => ExitCode::from(2),
            Self::ToolNotFound { .. } => ExitCode::from(3),
            Self::ToolFailed { .. } => ExitCode::from(4),
            Self::Timeout { .. } => ExitCode::from(5),
            Self::WithCode { code, .. } => ExitCode::from(*code),
            Self::AuditFailed => ExitCode::from(6),
            Self::Other(_) => ExitCode::from(1),
        }
    }
}
