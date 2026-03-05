//! Exit-code and stderr policy for protocol commands.
//!
//! Protocol commands use a three-tier exit-code scheme:
//! - Exit 0: command succeeded, status communicated via stdout (ready, blocked, etc.)
//! - Exit 1: operational failure (config not found, tool missing, parse error)
//! - Exit 2: usage error (bad arguments — handled by clap before we get here)
//!
//! Key principle: agents branch on stdout status fields (ready/blocked/etc.),
//! NOT on shell exit codes. Exit 0 means "I produced valid guidance output";
//! the status field within that output tells you what to do.
//!
//! Stderr is reserved for true operational errors (exit 1/2). Status information
//! like "blocked" or "needs-review" goes to stdout as part of the guidance output.

use std::process::ExitCode;

use super::render::{ProtocolGuidance, ProtocolStatus};
use crate::commands::doctor::OutputFormat;

/// Exit codes for protocol commands.
///
/// These are intentionally a small, fixed set. Agents should NOT branch on
/// these codes — they exist for shell-level error detection only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProtocolExitCode {
    /// Command produced valid guidance output (status in stdout).
    Success = 0,
    /// Operational failure: config missing, tool unavailable, parse error.
    OperationalError = 1,
    /// Usage error: bad arguments. (Typically handled by clap.)
    UsageError = 2,
}

impl From<ProtocolExitCode> for ExitCode {
    fn from(code: ProtocolExitCode) -> ExitCode {
        ExitCode::from(code as u8)
    }
}

/// Error type for protocol commands that need to set a specific exit code.
///
/// This integrates with the ExitError pattern in main.rs so that protocol
/// commands can signal operational failures (exit 1) via the standard
/// error-handling path.
#[derive(Debug, thiserror::Error)]
#[error("edict protocol: {context}: {detail}")]
pub struct ProtocolExitError {
    pub code: ProtocolExitCode,
    pub context: String,
    pub detail: String,
}

impl ProtocolExitError {
    /// Create an operational error (exit 1).
    pub fn operational(context: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: ProtocolExitCode::OperationalError,
            context: context.into(),
            detail: detail.into(),
        }
    }

    /// Convert to an ExitError for main.rs error handling.
    pub fn into_exit_error(self) -> crate::error::ExitError {
        crate::error::ExitError::new(self.code as u8, self.to_string())
    }
}

/// Result of a protocol command execution.
///
/// Bundles the guidance output with the appropriate exit code.
/// The caller (main.rs) uses this to print output and set the process exit code.
#[allow(dead_code)]
pub struct ProtocolResult {
    pub exit_code: ProtocolExitCode,
    pub guidance: Option<ProtocolGuidance>,
}

impl ProtocolResult {
    /// Command succeeded — guidance is ready to render.
    #[allow(dead_code)]
    pub fn success(guidance: ProtocolGuidance) -> Self {
        Self {
            exit_code: ProtocolExitCode::Success,
            guidance: Some(guidance),
        }
    }

    /// Operational error — no guidance produced.
    /// The error message will be written to stderr by the caller.
    #[allow(dead_code)]
    pub fn operational_error() -> Self {
        Self {
            exit_code: ProtocolExitCode::OperationalError,
            guidance: None,
        }
    }
}

/// All ProtocolStatus variants map to exit code 0 (Success).
///
/// This is the key design decision: blocked, needs-review, etc. are all
/// valid guidance states, not errors. The agent reads the status field
/// in stdout to decide what to do next.
#[allow(dead_code)]
pub fn exit_code_for_status(_status: ProtocolStatus) -> ProtocolExitCode {
    // Every status is a successful guidance output.
    // Agents branch on the status field, not the exit code.
    ProtocolExitCode::Success
}

/// Write a diagnostic message to stderr for operational errors.
///
/// Only call this for exit code 1 (operational failures).
/// Never use stderr for status information like "blocked" or "clean".
#[allow(dead_code)]
pub fn write_stderr_diagnostic(context: &str, detail: &str) {
    eprintln!("edict protocol: {context}: {detail}");
}

/// Render guidance to stdout and return the appropriate exit code.
///
/// This is the single exit path for all protocol commands that produce
/// valid guidance. The exit code is always 0 (Success) because the
/// guidance itself contains the status information.
#[allow(dead_code)]
pub fn render_and_exit(
    guidance: &ProtocolGuidance,
    format: OutputFormat,
) -> anyhow::Result<ProtocolExitCode> {
    let output = super::render::render(guidance, format)
        .map_err(|e| anyhow::anyhow!("render error: {}", e))?;
    println!("{}", output);
    Ok(exit_code_for_status(guidance.status))
}

/// Render guidance to stdout and return Ok(()).
///
/// Convenience wrapper around `render_and_exit` for commands that return
/// `anyhow::Result<()>`. All ProtocolStatus variants produce exit 0.
/// Operational errors should use `ProtocolExitError` instead.
pub fn render_guidance(guidance: &ProtocolGuidance, format: OutputFormat) -> anyhow::Result<()> {
    let output = super::render::render(guidance, format)
        .map_err(|e| anyhow::anyhow!("render error: {}", e))?;
    println!("{}", output);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::protocol::render::ProtocolGuidance;

    #[test]
    fn all_statuses_map_to_success() {
        let statuses = vec![
            ProtocolStatus::Ready,
            ProtocolStatus::Blocked,
            ProtocolStatus::Resumable,
            ProtocolStatus::NeedsReview,
            ProtocolStatus::HasResources,
            ProtocolStatus::Clean,
            ProtocolStatus::HasWork,
            ProtocolStatus::Fresh,
        ];
        for status in statuses {
            assert_eq!(
                exit_code_for_status(status),
                ProtocolExitCode::Success,
                "status {:?} should map to Success",
                status
            );
        }
    }

    #[test]
    fn exit_code_values() {
        assert_eq!(ProtocolExitCode::Success as u8, 0);
        assert_eq!(ProtocolExitCode::OperationalError as u8, 1);
        assert_eq!(ProtocolExitCode::UsageError as u8, 2);
    }

    #[test]
    fn exit_code_to_std_exit_code() {
        let code: ExitCode = ProtocolExitCode::Success.into();
        // ExitCode doesn't implement PartialEq, but we can verify the conversion compiles
        let _ = code;

        let code: ExitCode = ProtocolExitCode::OperationalError.into();
        let _ = code;

        let code: ExitCode = ProtocolExitCode::UsageError.into();
        let _ = code;
    }

    #[test]
    fn protocol_result_success() {
        let guidance = ProtocolGuidance::new("start");
        let result = ProtocolResult::success(guidance);
        assert_eq!(result.exit_code, ProtocolExitCode::Success);
        assert!(result.guidance.is_some());
    }

    #[test]
    fn protocol_result_operational_error() {
        let result = ProtocolResult::operational_error();
        assert_eq!(result.exit_code, ProtocolExitCode::OperationalError);
        assert!(result.guidance.is_none());
    }

    #[test]
    fn blocked_status_still_exits_zero() {
        let mut guidance = ProtocolGuidance::new("start");
        guidance.blocked("bone claimed by another agent".to_string());
        assert_eq!(
            exit_code_for_status(guidance.status),
            ProtocolExitCode::Success
        );
    }

    #[test]
    fn needs_review_status_still_exits_zero() {
        let mut guidance = ProtocolGuidance::new("review");
        guidance.status = ProtocolStatus::NeedsReview;
        assert_eq!(
            exit_code_for_status(guidance.status),
            ProtocolExitCode::Success
        );
    }

    #[test]
    fn has_resources_status_still_exits_zero() {
        let mut guidance = ProtocolGuidance::new("cleanup");
        guidance.status = ProtocolStatus::HasResources;
        assert_eq!(
            exit_code_for_status(guidance.status),
            ProtocolExitCode::Success
        );
    }

    #[test]
    fn protocol_exit_error_operational() {
        let err = ProtocolExitError::operational("start", "config not found");
        assert_eq!(err.code, ProtocolExitCode::OperationalError);
        assert_eq!(err.context, "start");
        assert_eq!(err.detail, "config not found");
        let msg = err.to_string();
        assert!(msg.contains("edict protocol: start: config not found"));
    }

    #[test]
    fn protocol_exit_error_to_exit_error() {
        let err = ProtocolExitError::operational("cleanup", "bus not available");
        let exit_err = err.into_exit_error();
        // ExitError::WithCode { code: 1, message: ... }
        assert_eq!(exit_err.exit_code(), ExitCode::from(1u8));
    }

    #[test]
    fn operational_error_exit_code_is_one() {
        let err = ProtocolExitError::operational("start", "tool missing");
        assert_eq!(err.code as u8, 1);
    }

    #[test]
    fn stderr_diagnostic_format() {
        // Just verify write_stderr_diagnostic doesn't panic.
        // The actual stderr output is tested via integration tests.
        write_stderr_diagnostic("start", "config not found");
    }
}
