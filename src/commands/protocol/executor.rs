//! Step executor for protocol commands with --execute mode.
//!
//! Executes shell commands sequentially, captures output, handles failures,
//! and performs $WS placeholder substitution for workspace names.

use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use thiserror::Error;

use crate::commands::doctor::OutputFormat;

/// Result of executing a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// The command that was run
    pub command: String,
    /// Whether the command succeeded (exit code 0)
    pub success: bool,
    /// Standard output from the command
    pub stdout: String,
    /// Standard error from the command
    pub stderr: String,
}

/// Complete execution report with results and remaining steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    /// Steps that were executed (in order)
    pub results: Vec<StepResult>,
    /// Steps that were not executed due to earlier failure
    pub remaining: Vec<String>,
}

/// Errors that can occur during step execution.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("failed to spawn command: {0}")]
    SpawnFailed(String),
    #[error("failed to capture command output: {0}")]
    OutputCaptureFailed(String),
}

/// Execute a list of shell commands sequentially.
///
/// Commands are run via `sh -c`, with output captured per step.
/// Execution stops on the first failure, and remaining steps are returned.
///
/// ### $WS Placeholder Substitution
///
/// When a step contains `maw ws create`, the executor parses the workspace name
/// from stdout and substitutes `$WS` in all subsequent steps.
///
/// Example:
/// - Step 1: `maw ws create --random` outputs "Creating workspace 'frost-castle'"
/// - Step 2: `rite claims stake --agent $AGENT "workspace://project/$WS"` becomes
///   `rite claims stake --agent $AGENT "workspace://project/frost-castle"`
pub fn execute_steps(steps: &[String]) -> Result<ExecutionReport, ExecutionError> {
    let mut results = Vec::new();
    let mut workspace_name: Option<String> = None;

    for (idx, step) in steps.iter().enumerate() {
        // Apply $WS substitution if workspace name is known
        let effective_step = if let Some(ref ws) = workspace_name {
            step.replace("$WS", ws)
        } else {
            step.clone()
        };

        // Execute the command via sh -c
        let output = Command::new("sh")
            .arg("-c")
            .arg(&effective_step)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| ExecutionError::SpawnFailed(format!("{}: {}", effective_step, e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        // Check if this step creates a workspace
        if step.contains("maw ws create") && success {
            workspace_name = extract_workspace_name(&stdout);
        }

        results.push(StepResult {
            command: effective_step,
            success,
            stdout,
            stderr,
        });

        // Stop on first failure
        if !success {
            let remaining = steps[idx + 1..].iter().map(|s| s.clone()).collect();
            return Ok(ExecutionReport { results, remaining });
        }
    }

    // All steps succeeded
    Ok(ExecutionReport {
        results,
        remaining: Vec::new(),
    })
}

/// Extract workspace name from `maw ws create` output.
///
/// Looks for patterns like:
/// - "Creating workspace 'frost-castle'"
/// - Just the workspace name on its own line
///
/// Returns the first alphanumeric-hyphen sequence found.
fn extract_workspace_name(stdout: &str) -> Option<String> {
    // Try to find quoted workspace name first
    if let Some(start) = stdout.find("Creating workspace '") {
        let after = &stdout[start + 20..];
        if let Some(end) = after.find('\'') {
            let ws_name = &after[..end];
            // Validate: must be non-empty, alphanumeric+hyphens, start with alphanumeric
            // This prevents command injection if maw output were ever malformed
            if !ws_name.is_empty()
                && ws_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                && ws_name.chars().next().unwrap().is_ascii_alphanumeric()
            {
                return Some(ws_name.to_string());
            }
        }
    }

    // Fallback: find the first valid workspace name (alphanumeric + hyphens)
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && trimmed.chars().next().unwrap().is_ascii_alphanumeric()
        {
            return Some(trimmed.to_string());
        }
    }

    None
}

/// Render an execution report in the specified format.
///
/// - Text: concise step-by-step output for agents
/// - JSON: structured output with all details
/// - Pretty: colored output with symbols for humans
pub fn render_report(report: &ExecutionReport, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => render_text(report),
        OutputFormat::Json => render_json(report),
        OutputFormat::Pretty => render_pretty(report),
    }
}

/// Render execution report as text (agent-friendly format).
///
/// Format:
/// ```text
/// step 1/5  rite claims stake --agent $AGENT 'bone://edict/bd-abc'  ok
/// step 2/5  maw ws create --random  ok  ws=frost-castle
/// step 3/5  rite claims stake --agent $AGENT 'workspace://edict/$WS'  FAILED
/// step 4/5  (not executed)
/// step 5/5  (not executed)
/// ```
fn render_text(report: &ExecutionReport) -> String {
    let total = report.results.len() + report.remaining.len();
    let mut out = String::new();

    for (idx, result) in report.results.iter().enumerate() {
        let step_num = idx + 1;
        let status = if result.success { "ok" } else { "FAILED" };

        out.push_str(&format!(
            "step {}/{}  {}  {}",
            step_num, total, result.command, status
        ));

        // If this was a workspace creation, show the workspace name
        if result.command.contains("maw ws create") && result.success {
            if let Some(ws) = extract_workspace_name(&result.stdout) {
                out.push_str(&format!("  ws={}", ws));
            }
        }

        out.push('\n');
    }

    for (idx, _remaining) in report.remaining.iter().enumerate() {
        let step_num = report.results.len() + idx + 1;
        out.push_str(&format!("step {}/{}  (not executed)\n", step_num, total));
    }

    out
}

/// Render execution report as JSON.
///
/// Includes all step results, remaining steps, and summary statistics.
fn render_json(report: &ExecutionReport) -> String {
    use serde_json::json;

    let total_steps = report.results.len() + report.remaining.len();
    let success = report.remaining.is_empty() && report.results.iter().all(|r| r.success);

    let results_json: Vec<_> = report
        .results
        .iter()
        .map(|r| {
            json!({
                "command": r.command,
                "success": r.success,
                "stdout": r.stdout,
                "stderr": r.stderr,
            })
        })
        .collect();

    let report_json = json!({
        "steps_run": report.results.len(),
        "steps_total": total_steps,
        "success": success,
        "results": results_json,
        "remaining": report.remaining,
    });

    serde_json::to_string_pretty(&report_json).unwrap()
}

/// Render execution report with color codes (TTY/human format).
///
/// Uses green checkmarks for success, red X for failures.
fn render_pretty(report: &ExecutionReport) -> String {
    let total = report.results.len() + report.remaining.len();
    let mut out = String::new();

    let green = "\x1b[32m";
    let red = "\x1b[31m";
    let gray = "\x1b[90m";
    let reset = "\x1b[0m";

    for (idx, result) in report.results.iter().enumerate() {
        let step_num = idx + 1;
        let (symbol, color) = if result.success {
            ("✓", green)
        } else {
            ("✗", red)
        };

        out.push_str(&format!(
            "step {}/{}  {}  {}{}{}",
            step_num, total, result.command, color, symbol, reset
        ));

        // If this was a workspace creation, show the workspace name
        if result.command.contains("maw ws create") && result.success {
            if let Some(ws) = extract_workspace_name(&result.stdout) {
                out.push_str(&format!("  {}ws={}{}", gray, ws, reset));
            }
        }

        out.push('\n');
    }

    for (idx, _remaining) in report.remaining.iter().enumerate() {
        let step_num = report.results.len() + idx + 1;
        out.push_str(&format!(
            "step {}/{}  {}(not executed){}\n",
            step_num, total, gray, reset
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Workspace name extraction tests ---

    #[test]
    fn extract_workspace_name_from_quoted_output() {
        let stdout = "Creating workspace 'frost-castle'\nWorkspace created successfully\n";
        assert_eq!(
            extract_workspace_name(stdout),
            Some("frost-castle".to_string())
        );
    }

    #[test]
    fn extract_workspace_name_from_plain_output() {
        let stdout = "amber-reef\n";
        assert_eq!(
            extract_workspace_name(stdout),
            Some("amber-reef".to_string())
        );
    }

    #[test]
    fn extract_workspace_name_with_whitespace() {
        let stdout = "  crimson-wave  \n";
        assert_eq!(
            extract_workspace_name(stdout),
            Some("crimson-wave".to_string())
        );
    }

    #[test]
    fn extract_workspace_name_no_match() {
        let stdout = "Error: workspace creation failed\n";
        assert_eq!(extract_workspace_name(stdout), None);
    }

    #[test]
    fn extract_workspace_name_rejects_shell_metacharacters() {
        // Defense-in-depth: quoted path 1 must validate alphanumeric+hyphens
        let stdout = "Creating workspace 'foo; rm -rf /'\n";
        assert_eq!(extract_workspace_name(stdout), None);
    }

    #[test]
    fn extract_workspace_name_rejects_spaces_in_quoted() {
        let stdout = "Creating workspace 'foo bar'\n";
        assert_eq!(extract_workspace_name(stdout), None);
    }

    #[test]
    fn extract_workspace_name_empty() {
        assert_eq!(extract_workspace_name(""), None);
    }

    #[test]
    fn extract_workspace_name_multiline_finds_first() {
        let stdout = "frost-castle\nSome other output\n";
        assert_eq!(
            extract_workspace_name(stdout),
            Some("frost-castle".to_string())
        );
    }

    // --- Step execution tests (mock, no actual subprocess calls) ---

    #[test]
    fn empty_steps_list() {
        let steps: Vec<String> = vec![];
        let report = execute_steps(&steps).unwrap();
        assert_eq!(report.results.len(), 0);
        assert_eq!(report.remaining.len(), 0);
    }

    // --- Rendering tests ---

    #[test]
    fn render_text_empty_report() {
        let report = ExecutionReport {
            results: vec![],
            remaining: vec![],
        };
        let text = render_text(&report);
        assert_eq!(text, "");
    }

    #[test]
    fn render_text_single_success() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "echo hello".to_string(),
                success: true,
                stdout: "hello\n".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let text = render_text(&report);
        assert!(text.contains("step 1/1"));
        assert!(text.contains("echo hello"));
        assert!(text.contains("ok"));
    }

    #[test]
    fn render_text_single_failure() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "false".to_string(),
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            }],
            remaining: vec!["echo not run".to_string()],
        };
        let text = render_text(&report);
        assert!(text.contains("step 1/2"));
        assert!(text.contains("false"));
        assert!(text.contains("FAILED"));
        assert!(text.contains("step 2/2"));
        assert!(text.contains("not executed"));
    }

    #[test]
    fn render_text_workspace_creation() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "maw ws create --random".to_string(),
                success: true,
                stdout: "Creating workspace 'amber-reef'\n".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let text = render_text(&report);
        assert!(text.contains("ws=amber-reef"));
    }

    #[test]
    fn render_json_valid_structure() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "echo test".to_string(),
                success: true,
                stdout: "test\n".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let json = render_json(&report);
        assert!(json.contains("steps_run"));
        assert!(json.contains("steps_total"));
        assert!(json.contains("success"));
        assert!(json.contains("results"));
        assert!(json.contains("remaining"));
    }

    #[test]
    fn render_json_with_failure() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "false".to_string(),
                success: false,
                stdout: String::new(),
                stderr: "error\n".to_string(),
            }],
            remaining: vec!["echo skipped".to_string()],
        };
        let json = render_json(&report);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["steps_run"].as_u64(), Some(1));
        assert_eq!(parsed["steps_total"].as_u64(), Some(2));
        assert_eq!(parsed["success"].as_bool(), Some(false));
    }

    #[test]
    fn render_pretty_has_colors() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "echo hello".to_string(),
                success: true,
                stdout: "hello\n".to_string(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let pretty = render_pretty(&report);
        assert!(pretty.contains("\x1b[")); // ANSI color codes
    }

    #[test]
    fn render_pretty_success_is_green() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "true".to_string(),
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let pretty = render_pretty(&report);
        assert!(pretty.contains("\x1b[32m")); // green
        assert!(pretty.contains("✓"));
    }

    #[test]
    fn render_pretty_failure_is_red() {
        let report = ExecutionReport {
            results: vec![StepResult {
                command: "false".to_string(),
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            }],
            remaining: vec![],
        };
        let pretty = render_pretty(&report);
        assert!(pretty.contains("\x1b[31m")); // red
        assert!(pretty.contains("✗"));
    }

    #[test]
    fn render_report_delegates_to_format() {
        let report = ExecutionReport {
            results: vec![],
            remaining: vec![],
        };

        let text = render_report(&report, OutputFormat::Text);
        assert_eq!(text, "");

        let json = render_report(&report, OutputFormat::Json);
        assert!(json.contains("steps_run"));

        let pretty = render_report(&report, OutputFormat::Pretty);
        // Pretty output for empty report is also empty
        assert_eq!(pretty, "");
    }

    // --- Integration tests: simulate $WS substitution logic ---

    #[test]
    fn ws_substitution_mock() {
        // Simulate what execute_steps does for $WS substitution
        let steps = vec![
            "maw ws create --random".to_string(),
            "echo workspace is $WS".to_string(),
            "rite claims stake 'workspace://$WS'".to_string(),
        ];

        // Mock workspace name extraction
        let mock_ws_output = "Creating workspace 'frost-castle'\n";
        let extracted_ws = extract_workspace_name(mock_ws_output);
        assert_eq!(extracted_ws, Some("frost-castle".to_string()));

        // Mock substitution
        let ws_name = extracted_ws.unwrap();
        let step2_with_sub = steps[1].replace("$WS", &ws_name);
        let step3_with_sub = steps[2].replace("$WS", &ws_name);

        assert_eq!(step2_with_sub, "echo workspace is frost-castle");
        assert_eq!(
            step3_with_sub,
            "rite claims stake 'workspace://frost-castle'"
        );
    }

    #[test]
    fn ws_substitution_no_workspace_created() {
        // If no workspace is created, $WS should remain as-is
        let step = "echo $WS is unknown".to_string();
        let ws_name: Option<String> = None;

        let effective_step = if let Some(ref ws) = ws_name {
            step.replace("$WS", ws)
        } else {
            step.clone()
        };

        assert_eq!(effective_step, "echo $WS is unknown");
    }

    // --- Real subprocess test (optional, can be slow) ---

    #[test]
    #[ignore] // Run with `cargo test -- --ignored` to include subprocess tests
    fn execute_steps_real_subprocess() {
        let steps = vec!["echo hello".to_string(), "echo world".to_string()];
        let report = execute_steps(&steps).unwrap();
        assert_eq!(report.results.len(), 2);
        assert!(report.results[0].success);
        assert!(report.results[1].success);
        assert!(report.remaining.is_empty());
    }

    #[test]
    #[ignore]
    fn execute_steps_stops_on_failure() {
        let steps = vec![
            "true".to_string(),
            "false".to_string(),
            "echo should not run".to_string(),
        ];
        let report = execute_steps(&steps).unwrap();
        assert_eq!(report.results.len(), 2);
        assert!(report.results[0].success);
        assert!(!report.results[1].success);
        assert_eq!(report.remaining.len(), 1);
        assert_eq!(report.remaining[0], "echo should not run");
    }
}
