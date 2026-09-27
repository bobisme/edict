//! Shared spawner for the inner `edict run agent` child process.
//!
//! The dev-loop, worker-loop, and responder all run an agent turn by spawning
//! `edict run agent` as a child. The child's stdout is streamed and captured
//! for parsing; its stderr is passed through live and a bounded tail is kept,
//! so a failure carries the child's real reason (for example the `ToolFailed`
//! message `main` prints) instead of just an exit code.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use anyhow::Context;

/// Number of trailing stderr lines kept from the child for error reporting.
const STDERR_TAIL_LINES: usize = 20;

/// Where the child's stdout lines are echoed while being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoTo {
    Stdout,
    Stderr,
}

/// Run `edict run agent` with `args` (everything after `edict`).
///
/// # Errors
///
/// Returns `Err` if the child cannot be spawned or exits non-zero. On non-zero
/// exit the error's first line is `<reason> (edict run agent exited with code N)`,
/// where `<reason>` is the most meaningful line of the child's stderr.
pub fn run_edict_agent<S: AsRef<OsStr>>(args: &[S], echo: EchoTo) -> anyhow::Result<String> {
    run_agent_command("edict", args, echo)
}

/// Spawn `program args...`, stream/capture stdout, pass stderr through while
/// keeping a bounded tail, and build a descriptive error on failure.
///
/// Split out from [`run_edict_agent`] so tests can substitute a fake program.
fn run_agent_command<P: AsRef<OsStr>, S: AsRef<OsStr>>(
    program: P,
    args: &[S],
    echo: EchoTo,
) -> anyhow::Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning edict run agent")?;

    let stderr = child.stderr.take().context("capturing stderr")?;
    // Drain stderr on its own thread so a chatty child can't deadlock us by
    // filling the stderr pipe while we block reading stdout.
    let stderr_thread = std::thread::spawn(move || {
        let mut tail: VecDeque<String> = VecDeque::with_capacity(STDERR_TAIL_LINES);
        let reader = BufReader::new(stderr);
        for line in reader.split(b'\n') {
            let Ok(bytes) = line else { break };
            let line = String::from_utf8_lossy(&bytes);
            let line = line.trim_end_matches('\r');
            // Pass through live so operators still see it.
            eprintln!("{line}");
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line.to_string());
        }
        tail
    });

    let stdout = child.stdout.take().context("capturing stdout")?;
    let mut output = String::new();
    let mut read_err = None;
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) => {
                match echo {
                    EchoTo::Stdout => println!("{line}"),
                    EchoTo::Stderr => eprintln!("{line}"),
                }
                output.push_str(&line);
                output.push('\n');
            }
            Err(e) => {
                read_err = Some(e);
                break;
            }
        }
    }

    let status = child.wait().context("waiting for edict run agent")?;
    let tail: Vec<String> = stderr_thread.join().map(Vec::from).unwrap_or_default();

    if let Some(e) = read_err {
        return Err(anyhow::Error::new(e).context("reading stdout line from edict run agent"));
    }

    if status.success() {
        return Ok(output);
    }

    let code = status.code().unwrap_or(-1);
    Err(anyhow::anyhow!(failure_message(&tail, code)))
}

/// Build the failure message: reason first (so single-line consumers like the
/// responder show it), then the exit code. Keeps the phrase "exited with code"
/// so the worker's model fallback continues to recognise it.
fn failure_message<S: AsRef<str>>(stderr_tail: &[S], code: i32) -> String {
    let base = format!("edict run agent exited with code {code}");
    meaningful_error_line(stderr_tail)
        .map_or_else(|| base.clone(), |reason| format!("{reason} ({base})"))
}

/// Pick the most meaningful line from a child's stderr tail.
///
/// Prefers the last line starting with `error`/`Error` (as printed by
/// `main`), with its `error:` prefix stripped; otherwise the last non-empty
/// line. Returns `None` when there is nothing useful.
fn meaningful_error_line<S: AsRef<str>>(lines: &[S]) -> Option<String> {
    let non_empty = || {
        lines
            .iter()
            .map(|l| l.as_ref().trim())
            .filter(|l| !l.is_empty())
    };

    let chosen = non_empty()
        .rev()
        .find(|l| l.to_ascii_lowercase().starts_with("error"))
        .or_else(|| non_empty().last())?;

    let stripped = strip_error_prefix(chosen);
    let reason = if stripped.is_empty() {
        chosen
    } else {
        stripped
    };
    Some(reason.to_string())
}

fn strip_error_prefix(line: &str) -> &str {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("error:") {
        line["error:".len()..].trim_start()
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn picks_last_error_line_and_strips_prefix() {
        let lines = [
            "Trying model 1/1",
            "error: first thing",
            "some noise",
            "error: claude failed (exit 1): auth expired",
            "  continuation detail",
            "",
        ];
        assert_eq!(
            meaningful_error_line(&lines).as_deref(),
            Some("claude failed (exit 1): auth expired")
        );
    }

    #[test]
    fn matches_capitalised_error_prefix() {
        let lines = ["hello", "Error: Something broke", "bye"];
        assert_eq!(
            meaningful_error_line(&lines).as_deref(),
            Some("Something broke")
        );
    }

    #[test]
    fn keeps_non_colon_error_lines_verbatim() {
        let lines = ["ErrorKind::NotFound while opening config"];
        assert_eq!(
            meaningful_error_line(&lines).as_deref(),
            Some("ErrorKind::NotFound while opening config")
        );
    }

    #[test]
    fn falls_back_to_last_non_empty_line() {
        let lines = ["starting", "panicked at src/x.rs:1", "   ", ""];
        assert_eq!(
            meaningful_error_line(&lines).as_deref(),
            Some("panicked at src/x.rs:1")
        );
    }

    #[test]
    fn bare_error_prefix_is_not_emptied() {
        let lines = ["error:"];
        assert_eq!(meaningful_error_line(&lines).as_deref(), Some("error:"));
    }

    #[test]
    fn nothing_useful_returns_none() {
        let empty: [&str; 0] = [];
        assert_eq!(meaningful_error_line(&empty), None);
        assert_eq!(meaningful_error_line(&["", "  "]), None);
    }

    #[test]
    fn failure_message_puts_reason_on_first_line() {
        let msg = failure_message(&["error: boom"], 4);
        assert_eq!(msg, "boom (edict run agent exited with code 4)");
        assert!(msg.contains("exited with code"));
        assert_eq!(
            failure_message::<&str>(&[], 2),
            "edict run agent exited with code 2"
        );
    }

    /// Write a fake runner script. Tests run it as `/bin/sh <script> ...`
    /// rather than exec'ing it directly: exec'ing a just-written file races
    /// with concurrent test threads forking (ETXTBSY, "Text file busy").
    fn fake_runner(script: &str) -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fake-edict");
        fs::write(&path, script).unwrap();
        (tmp, path.to_string_lossy().into_owned())
    }

    #[test]
    fn propagates_inner_error_text_on_failure() {
        let (_tmp, path) = fake_runner(
            "#!/bin/sh\n\
             echo 'partial stdout'\n\
             echo 'Trying model 1/1' >&2\n\
             echo 'error: claude failed (exit 1): Invalid API key' >&2\n\
             exit 4\n",
        );
        let err = run_agent_command(
            "/bin/sh",
            &[path.as_str(), "run", "agent", "hi"],
            EchoTo::Stderr,
        )
        .unwrap_err();
        let msg = err.to_string();
        let first = msg.lines().next().unwrap();
        assert_eq!(
            first,
            "claude failed (exit 1): Invalid API key (edict run agent exited with code 4)"
        );
    }

    #[test]
    fn returns_stdout_on_success() {
        let (_tmp, path) =
            fake_runner("#!/bin/sh\necho 'line one'\necho 'noise' >&2\necho 'line two'\n");
        let out = run_agent_command("/bin/sh", &[path.as_str(), "x"], EchoTo::Stderr).unwrap();
        assert_eq!(out, "line one\nline two\n");
    }

    #[test]
    fn large_stderr_does_not_deadlock_and_keeps_tail() {
        // Write far more than a pipe buffer to stderr before any stdout, then fail.
        let (_tmp, path) = fake_runner(
            "#!/bin/sh\n\
             i=0\n\
             while [ $i -lt 2000 ]; do echo \"noise line $i padding padding padding\" >&2; i=$((i+1)); done\n\
             echo 'done'\n\
             echo 'error: final reason' >&2\n\
             exit 1\n",
        );
        let err = run_agent_command("/bin/sh", &[path.as_str(), "x"], EchoTo::Stderr).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("final reason (edict run agent exited with code 1)")
        );
    }

    #[test]
    fn failure_without_stderr_keeps_plain_message() {
        let (_tmp, path) = fake_runner("#!/bin/sh\nexit 3\n");
        let err = run_agent_command("/bin/sh", &[path.as_str(), "x"], EchoTo::Stdout).unwrap_err();
        assert_eq!(err.to_string(), "edict run agent exited with code 3");
    }
}
