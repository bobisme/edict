use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::Context;

use crate::effects::Effects;
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
    #[must_use]
    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Parse stdout as JSON.
    ///
    /// # Errors
    ///
    /// Returns `Err` if stdout is not valid JSON for the target type.
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
    /// When true, spawn the subprocess in a new process group (`process_group(0)`) so
    /// it survives a SIGTERM directed at the parent's process group.  Use this for
    /// cleanup subprocesses that must outlive the signal that triggered them.
    new_process_group: bool,
}

impl Tool {
    /// Create a new tool invocation.
    #[must_use]
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
    /// (e.g. `rite claims release`) that are spawned from a signal handler.
    ///
    /// On non-Unix platforms this is a no-op (the flag is ignored).
    #[must_use]
    pub const fn new_process_group(mut self) -> Self {
        self.new_process_group = true;
        self
    }

    /// Add a single argument.
    #[must_use]
    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    /// Add multiple arguments.
    #[must_use]
    pub fn args(mut self, args: &[&str]) -> Self {
        self.args
            .extend(args.iter().map(std::string::ToString::to_string));
        self
    }

    /// Set a timeout for the subprocess.
    #[allow(dead_code)]
    #[must_use]
    pub const fn timeout(mut self, duration: Duration) -> Self {
        self.timeout = Some(duration);
        self
    }

    /// Wrap this command with `maw exec <workspace> --`.
    ///
    /// Validates that the workspace name matches `[a-z0-9][a-z0-9-]*` to prevent
    /// argument confusion with the maw CLI.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the workspace name is empty, too long, or contains
    /// characters outside `[a-z0-9-]` or path components.
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
    ///
    /// # Errors
    ///
    /// Returns `Err` if the program cannot be spawned, is not found, or times out.
    #[tracing::instrument(skip(self), fields(tool = %self.program, workspace = ?self.maw_workspace))]
    pub fn run(&self) -> anyhow::Result<RunOutput> {
        let (program, args) = self.build_command();

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Detach cleanup subprocesses into their own process group so they
        // survive a SIGTERM that kills the parent's process group (e.g. from
        // `vessel kill`).  On non-Unix targets the flag is simply ignored.
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
    ///
    /// # Errors
    ///
    /// Returns `Err` if the tool cannot be run or exits with a non-zero status.
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

    /// The program and arguments this invocation would spawn (after any
    /// `maw exec` wrapping) — what a dry-run plan shows.
    #[must_use]
    pub fn command_line(&self) -> (String, Vec<String>) {
        self.build_command()
    }

    fn build_command(&self) -> (String, Vec<String>) {
        self.maw_workspace.as_ref().map_or_else(
            || (self.program.clone(), self.args.clone()),
            |ws| {
                let mut args = vec![
                    "exec".to_string(),
                    ws.clone(),
                    "--".to_string(),
                    self.program.clone(),
                ];
                args.extend(self.args.clone());
                ("maw".to_string(), args)
            },
        )
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

/// Ensure exactly one rite hook exists with the given description.
///
/// Converges the hook by its stable name (`rite hooks add --name`), which
/// updates the record in place and keeps its ID. That matters:
///
/// - The ID is the spawn-lease key (`spawn://<hook-id>/<channel>`). A new ID
///   means a responder that is still running holds a lease on the old ID while
///   the replacement sees a free pattern and spawns a second agent beside it.
/// - Fields edict does not pass keep their current values, so a converge cannot
///   strip configuration edict never learned about — a lease, most of all.
/// - `last_fired` survives, so a cooldown hook does not get a free firing.
///
/// Hooks registered before named hooks existed carry no name, and adding a
/// named hook beside one produces a duplicate rather than adopting it. Those
/// are removed once, on the converge that adopts the name.
///
/// Falls back to remove-and-add against a rite that has no `--name`.
///
/// The `add_args` slice should contain all args for `rite hooks add` *except*
/// `--description`, `--name` and `--owner`, which are added here.
///
/// Returns `Ok(("created"|"updated"|"adopted", hook_id))`.
///
/// # Errors
///
/// Returns `Err` if the `rite hooks add` command cannot be run or fails.
pub fn ensure_rite_hook(description: &str, add_args: &[&str]) -> anyhow::Result<(String, String)> {
    ensure_rite_hook_with(&Effects::apply(), description, add_args)
}

/// [`ensure_rite_hook`], with every mutating `rite` call routed through `fx`.
///
/// A dry-run records the converge instead of performing it. The `rite hooks
/// list` and `--help` probes that decide the plan run in both modes.
///
/// # Errors
///
/// Returns `Err` if the `rite hooks add` command cannot be run or fails.
pub fn ensure_rite_hook_with(
    fx: &Effects,
    description: &str,
    add_args: &[&str],
) -> anyhow::Result<(String, String)> {
    let hooks = list_rite_hooks();
    let named = rite_supports_named_hooks();
    let plan = plan_converge(&hooks, description, named);

    let mut adopted = false;
    let mut removed = false;
    match &plan {
        // Nothing to remove: the add updates the named record in place.
        ConvergePlan::UpdateInPlace | ConvergePlan::Create => {}
        ConvergePlan::Adopt { id, duplicates } => {
            let set_ok = fx
                .run(&Tool::new("rite").args(&[
                    "hooks",
                    "set",
                    id,
                    "--name",
                    description,
                    "--owner",
                    HOOK_OWNER,
                ]))
                .is_ok_and(|o| o.success());
            if set_ok {
                adopted = true;
            } else {
                // Could not name it in place — fall back to replacing it.
                let _ = fx.run(&Tool::new("rite").args(&["hooks", "remove", id]));
                removed = true;
            }
            for dup in duplicates {
                let _ = fx.run(&Tool::new("rite").args(&["hooks", "remove", dup]));
                removed = true;
            }
        }
        ConvergePlan::Replace(ids) => {
            for id in ids {
                let _ = fx.run(&Tool::new("rite").args(&["hooks", "remove", id]));
                removed = true;
            }
        }
    }

    let mut args = vec!["hooks", "add", "--description", description];
    if named {
        args.extend_from_slice(&["--name", description, "--owner", HOOK_OWNER]);
    }
    args.extend_from_slice(add_args);

    let intent = match &plan {
        ConvergePlan::UpdateInPlace => "update in place".to_string(),
        ConvergePlan::Adopt { id, .. } => format!("adopt {id}"),
        ConvergePlan::Replace(ids) => format!("replace {}", ids.join(", ")),
        ConvergePlan::Create => "create".to_string(),
    };
    let result = fx.run_as(Some(&intent), &Tool::new("rite").args(&args))?;

    if !result.success() {
        anyhow::bail!("rite hooks add failed: {}", result.stderr.trim());
    }

    // Extract hook ID from output (format: "Added: Hook hk-xxx created")
    let hook_id = result
        .stdout
        .split_whitespace()
        .find(|s| s.starts_with("hk-"))
        .unwrap_or("unknown")
        .to_string();

    let action = if plan == ConvergePlan::UpdateInPlace {
        "updated"
    } else if adopted {
        "adopted"
    } else if removed {
        "replaced"
    } else {
        "created"
    };
    Ok((action.to_string(), hook_id))
}

/// What a converge must do to the hooks already in the store.
#[derive(Debug, PartialEq, Eq)]
enum ConvergePlan {
    /// A hook already carries this name: adding again updates it in place.
    UpdateInPlace,
    /// A legacy hook matches by description. Name it in place, keeping its ID
    /// (and its spawn lease); remove any further duplicate, since a name is
    /// unique per channel.
    Adopt { id: String, duplicates: Vec<String> },
    /// No named-hook support: the old remove-and-add path.
    Replace(Vec<String>),
    /// Nothing matches — this is a new hook.
    Create,
}

/// Decide how to converge, given the current hooks and whether rite names them.
fn plan_converge(hooks: &[serde_json::Value], description: &str, named: bool) -> ConvergePlan {
    let field = |h: &serde_json::Value, key: &str| -> Option<String> {
        h.get(key)
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
    };

    if named
        && hooks
            .iter()
            .any(|h| field(h, "name").as_deref() == Some(description))
    {
        return ConvergePlan::UpdateInPlace;
    }

    let matching: Vec<String> = hooks
        .iter()
        .filter(|h| field(h, "description").as_deref() == Some(description))
        .filter(|h| !named || field(h, "name").is_none())
        .filter_map(|h| field(h, "id"))
        .collect();

    if !named {
        return if matching.is_empty() {
            ConvergePlan::Create
        } else {
            ConvergePlan::Replace(matching)
        };
    }

    let mut ids = matching.into_iter();
    ids.next()
        .map_or(ConvergePlan::Create, |id| ConvergePlan::Adopt {
            id,
            duplicates: ids.collect(),
        })
}

/// Owner recorded on every hook edict manages (`rite hooks list --owner edict`).
pub const HOOK_OWNER: &str = "edict";

/// Read the hook records rite knows about, or an empty list when rite is absent.
fn list_rite_hooks() -> Vec<serde_json::Value> {
    let Ok(output) = Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    serde_json::from_str::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|v| v.get("hooks").and_then(|h| h.as_array()).cloned())
        .unwrap_or_default()
}

/// Report whether the installed rite converges hooks by name.
///
/// Probed from `--help` rather than the version string: named hooks landed on
/// rite trunk after v0.33.0 was cut, so `rite --version` says 0.33.0 both with
/// and without them.
pub fn rite_supports_named_hooks() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| help_mentions("rite", &["hooks", "add", "--help"], "--name"))
}

/// Report whether the installed vessel inherits a whole env namespace
/// (`--env-inherit "RITE_*"`).
///
/// An older vessel treats `RITE_*` as a literal variable name and silently
/// inherits nothing, which would leave every spawned agent without
/// `RITE_CHANNEL`. Probe before shortening the list.
pub fn vessel_supports_env_prefix() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| help_mentions("vessel", &["spawn", "--help"], "RITE_*"))
}

/// Run `<tool> <args>` and report whether the help text contains `needle`.
fn help_mentions(tool: &str, args: &[&str], needle: &str) -> bool {
    Tool::new(tool)
        .args(args)
        .run()
        .is_ok_and(|o| o.stdout.contains(needle) || o.stderr.contains(needle))
}

/// Simple helper to run a command with args, optionally in a specific directory.
/// Returns stdout on success, or an error.
///
/// # Errors
///
/// Returns `Err` if the command cannot be run or exits with a non-zero status.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(id: &str, description: &str, name: Option<&str>) -> serde_json::Value {
        let mut h = serde_json::json!({"id": id, "description": description});
        if let Some(n) = name {
            h["name"] = serde_json::json!(n);
        }
        h
    }

    #[test]
    fn a_named_hook_converges_in_place() {
        // The whole point: no removal, so the ID — and the spawn lease keyed on
        // it — survives the converge.
        let hooks = vec![hook(
            "hk-1",
            "edict:demo:responder",
            Some("edict:demo:responder"),
        )];
        assert_eq!(
            plan_converge(&hooks, "edict:demo:responder", true),
            ConvergePlan::UpdateInPlace
        );
    }

    #[test]
    fn a_legacy_hook_is_named_in_place_not_replaced() {
        let hooks = vec![hook("hk-1", "edict:demo:responder", None)];
        assert_eq!(
            plan_converge(&hooks, "edict:demo:responder", true),
            ConvergePlan::Adopt {
                id: "hk-1".to_string(),
                duplicates: vec![],
            }
        );
    }

    #[test]
    fn duplicate_legacy_hooks_collapse_to_one() {
        let hooks = vec![
            hook("hk-1", "edict:demo:responder", None),
            hook("hk-2", "edict:demo:responder", None),
        ];
        assert_eq!(
            plan_converge(&hooks, "edict:demo:responder", true),
            ConvergePlan::Adopt {
                id: "hk-1".to_string(),
                duplicates: vec!["hk-2".to_string()],
            }
        );
    }

    #[test]
    fn other_hooks_are_never_touched() {
        let hooks = vec![
            hook(
                "hk-1",
                "edict:other:responder",
                Some("edict:other:responder"),
            ),
            hook("hk-2", "rite:canary", None),
        ];
        assert_eq!(
            plan_converge(&hooks, "edict:demo:responder", true),
            ConvergePlan::Create
        );
    }

    #[test]
    fn without_named_hooks_the_old_replace_path_stands() {
        let hooks = vec![hook("hk-1", "edict:demo:responder", None)];
        assert_eq!(
            plan_converge(&hooks, "edict:demo:responder", false),
            ConvergePlan::Replace(vec!["hk-1".to_string()])
        );
        assert_eq!(
            plan_converge(&[], "edict:demo:responder", false),
            ConvergePlan::Create
        );
    }

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
