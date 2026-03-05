use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::Context;

use crate::error::ExitError;

// On Unix, CommandExt lets us call .process_group(0) to detach the child
// into its own process group so SIGTERM to the parent's group doesn't kill it.
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

/// Result of running a subprocess.
#[derive(Debug)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl RunOutput {
    /// Returns true if the process exited successfully.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Parse stdout as JSON.
    pub fn parse_json<T: serde::de::DeserializeOwned>(&self) -> anyhow::Result<T> {
        serde_json::from_str(&self.stdout)
            .with_context(|| "parsing JSON output from subprocess".to_string())
    }
}

/// Builder for running companion tools.
pub struct Tool {
    program: String,
    args: Vec<String>,
    timeout: Option<Duration>,
    maw_workspace: Option<String>,
    /// When true, spawn the subprocess in a new process group (process_group(0)) so
    /// it survives a SIGTERM directed at the parent's process group.  Use this for
    /// cleanup subprocesses that must outlive the signal that triggered them.
    new_process_group: bool,
}

impl Tool {
    /// Create a new tool invocation.
    pub fn new(program: &str) -> Self {
        Self {
            program: program.to_string(),
            args: Vec::new(),
            timeout: None,
            maw_workspace: None,
            new_process_group: false,
        }
    }

    /// Spawn the subprocess in a new process group so it survives a SIGTERM
    /// sent to the parent's process group.  Use this for cleanup subprocesses
    /// (e.g. `bus claims release`) that are spawned from a signal handler.
    ///
    /// On non-Unix platforms this is a no-op (the flag is ignored).
    pub fn new_process_group(mut self) -> Self {
        self.new_process_group = true;
        self
    }

    /// Add a single argument.
    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    /// Add multiple arguments.
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args.extend(args.iter().map(|s| s.to_string()));
        self
    }

    /// Set a timeout for the subprocess.
    #[allow(dead_code)]
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Wrap this command with `maw exec <workspace> --`.
    ///
    /// Validates that the workspace name matches `[a-z0-9][a-z0-9-]*` to prevent
    /// argument confusion with the maw CLI.
    pub fn in_workspace(mut self, workspace: &str) -> anyhow::Result<Self> {
        if workspace.is_empty()
            || !workspace
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || workspace.starts_with('-')
            || workspace.contains("..")
            || workspace.contains('/')
            || workspace.len() > 64
        {
            anyhow::bail!(
                "invalid workspace name {workspace:?}: must match [a-z0-9][a-z0-9-]*, max 64 chars, no path components"
            );
        }
        self.maw_workspace = Some(workspace.to_string());
        Ok(self)
    }

    /// Run the tool, capturing stdout and stderr.
    #[tracing::instrument(skip(self), fields(tool = %self.program, workspace = ?self.maw_workspace))]
    pub fn run(&self) -> anyhow::Result<RunOutput> {
        let (program, args) = self.build_command();

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Detach cleanup subprocesses into their own process group so they
        // survive a SIGTERM that kills the parent's process group (e.g. from
        // `botty kill`).  On non-Unix targets the flag is simply ignored.
        #[cfg(unix)]
        if self.new_process_group {
            cmd.process_group(0);
        }

        let start = crate::telemetry::metrics::time_start();

        let output: Output = if let Some(timeout) = self.timeout {
            run_with_timeout(&mut cmd, timeout, &self.program)?
        } else {
            cmd.output().map_err(|e| self.not_found_or_other(e))?
        };

        let success = output.status.success();
        let tool_name = &self.program;
        let success_str = if success { "true" } else { "false" };
        crate::telemetry::metrics::time_record(
            "edict.subprocess.duration_seconds",
            start,
            &[("tool", tool_name), ("success", success_str)],
        );
        crate::telemetry::metrics::counter(
            "edict.subprocess.calls_total",
            1,
            &[("tool", tool_name), ("success", success_str)],
        );

        Ok(RunOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Run the tool and return an error if it fails.
    pub fn run_ok(&self) -> anyhow::Result<RunOutput> {
        let output = self.run()?;
        if output.success() {
            Ok(output)
        } else {
            Err(ExitError::ToolFailed {
                tool: self.program.clone(),
                code: output.exit_code,
                message: output.stderr.trim().to_string(),
            }
            .into())
        }
    }

    fn build_command(&self) -> (String, Vec<String>) {
        if let Some(ref ws) = self.maw_workspace {
            let mut args = vec![
                "exec".to_string(),
                ws.clone(),
                "--".to_string(),
                self.program.clone(),
            ];
            args.extend(self.args.clone());
            ("maw".to_string(), args)
        } else {
            (self.program.clone(), self.args.clone())
        }
    }

    fn not_found_or_other(&self, e: std::io::Error) -> anyhow::Error {
        if e.kind() == std::io::ErrorKind::NotFound {
            let tool = if self.maw_workspace.is_some() {
                "maw"
            } else {
                &self.program
            };
            ExitError::ToolNotFound {
                tool: tool.to_string(),
            }
            .into()
        } else {
            anyhow::Error::new(e).context(format!("running {}", self.program))
        }
    }
}

fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    tool_name: &str,
) -> anyhow::Result<Output> {
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::Error::from(ExitError::ToolNotFound {
                tool: tool_name.to_string(),
            })
        } else {
            anyhow::Error::new(e).context(format!("spawning {tool_name}"))
        }
    })?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process exited — collect output
                let stdout = child.stdout.take().map_or_else(Vec::new, |mut r| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut r, &mut buf).unwrap_or(0);
                    buf
                });
                let stderr = child.stderr.take().map_or_else(Vec::new, |mut r| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut r, &mut buf).unwrap_or(0);
                    buf
                });
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                // Still running
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ExitError::Timeout {
                        tool: tool_name.to_string(),
                        timeout_secs: timeout.as_secs(),
                    }
                    .into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(anyhow::Error::new(e).context(format!("waiting for {tool_name}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_echo() {
        let output = Tool::new("echo").arg("hello").run().unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[test]
    fn run_false_fails() {
        let output = Tool::new("false").run().unwrap();
        assert!(!output.success());
    }

    #[test]
    fn run_ok_returns_error_on_failure() {
        let result = Tool::new("false").run_ok();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.downcast_ref::<ExitError>().is_some());
    }

    #[test]
    fn run_not_found() {
        let result = Tool::new("nonexistent-tool-xyz").run();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let exit_err = err.downcast_ref::<ExitError>().unwrap();
        assert!(matches!(exit_err, ExitError::ToolNotFound { .. }));
    }

    #[test]
    fn run_with_timeout_succeeds() {
        let output = Tool::new("echo")
            .arg("fast")
            .timeout(Duration::from_secs(5))
            .run()
            .unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "fast");
    }

    #[test]
    fn maw_exec_wrapper() {
        // Verify command construction (won't actually run since maw may not be available)
        let tool = Tool::new("bn").arg("next").in_workspace("default").unwrap();
        let (program, args) = tool.build_command();
        assert_eq!(program, "maw");
        assert_eq!(args, vec!["exec", "default", "--", "bn", "next"]);
    }

    #[test]
    fn invalid_workspace_names() {
        assert!(Tool::new("bn").in_workspace("").is_err());
        assert!(Tool::new("bn").in_workspace("--flag").is_err());
        assert!(Tool::new("bn").in_workspace("-starts-dash").is_err());
        assert!(Tool::new("bn").in_workspace("Has Uppercase").is_err());
        assert!(Tool::new("bn").in_workspace("has space").is_err());
        // Valid names
        assert!(Tool::new("bn").in_workspace("default").is_ok());
        assert!(Tool::new("bn").in_workspace("northern-cedar").is_ok());
        assert!(Tool::new("bn").in_workspace("ws123").is_ok());
    }

    #[test]
    fn parse_json_output() {
        let output = RunOutput {
            stdout: r#"{"key": "value"}"#.to_string(),
            stderr: String::new(),
            exit_code: 0,
        };
        let parsed: serde_json::Value = output.parse_json().unwrap();
        assert_eq!(parsed["key"], "value");
    }
}

/// Ensure exactly one bus hook exists with the given description.
///
/// Performs idempotent upsert: finds any existing hook(s) matching the
/// description, removes them, then adds a new hook with current parameters.
/// The `add_args` slice should contain all args for `bus hooks add` *except*
/// `--description` (which is added automatically).
///
/// Returns `Ok(("created"|"updated"|"unchanged", hook_id))`.
pub fn ensure_bus_hook(description: &str, add_args: &[&str]) -> anyhow::Result<(String, String)> {
    // List existing hooks
    let existing = Tool::new("bus")
        .args(&["hooks", "list", "--format", "json"])
        .run();

    let mut removed = false;
    if let Ok(output) = existing {
        if output.success() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&output.stdout) {
                if let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) {
                    for hook in hooks {
                        let desc = hook.get("description").and_then(|d| d.as_str());
                        if desc == Some(description) {
                            if let Some(id) = hook.get("id").and_then(|i| i.as_str()) {
                                let _ = Tool::new("bus").args(&["hooks", "remove", id]).run();
                                removed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // Add with --description
    let mut args = vec!["hooks", "add", "--description", description];
    args.extend_from_slice(add_args);

    let result = Tool::new("bus").args(&args).run()?;

    if !result.success() {
        anyhow::bail!("bus hooks add failed: {}", result.stderr.trim());
    }

    // Extract hook ID from output (format: "Added: Hook hk-xxx created")
    let hook_id = result
        .stdout
        .split_whitespace()
        .find(|s| s.starts_with("hk-"))
        .unwrap_or("unknown")
        .to_string();

    let action = if removed { "updated" } else { "created" };
    Ok((action.to_string(), hook_id))
}

/// Simple helper to run a command with args, optionally in a specific directory.
/// Returns stdout on success, or an error.
pub fn run_command(program: &str, args: &[&str], cwd: Option<&Path>) -> anyhow::Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let output = cmd.output().with_context(|| format!("running {program}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        anyhow::bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}
