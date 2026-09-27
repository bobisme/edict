//! Integration test: `edict init` refuses to register a *live* rite hook
//! for a project rooted under the system temp directory when
//! `RITE_DATA_DIR` is unset (see `src/rite_hook_guard.rs`).
//!
//! Unlike `hermetic_rite_hooks.rs`, this test leaves `RITE_DATA_DIR`
//! *unset* on purpose — it's the guard's job to notice that and refuse,
//! rather than silently register a hook against the machine's real rite
//! data directory for what looks like a throwaway project.
//!
//! `HOME`/`XDG_DATA_HOME` are still pointed at a sentinel tempdir. This
//! matters for two reasons:
//!
//! - rite's default data dir derives from `HOME`/`XDG_DATA_HOME` (see
//!   `rite/src/core/project.rs::data_dir()`; confirmed empirically here by
//!   running `rite doctor`/`rite hooks list` with a substituted `HOME` and
//!   observing the printed/created path move with it). So if the guard were
//!   ever silently broken, the resulting live hook would land in this
//!   disposable sentinel directory, never in the real one.
//! - it lets this test check, positively, that the sentinel's rite data
//!   contains no hook after the refusal — not just that `edict init` failed
//!   for some unrelated reason.
//!
//! One nuance: `edict init --tools rite` also sends a project-registration
//! message to the `#projects` channel (`register_project_channel` in
//! `src/commands/init.rs`), and its preliminary `rite hooks list` calls
//! create rite's on-disk skeleton (state files) as a side effect. The
//! `#projects` send is guarded the same way as the hook registration itself
//! (`rite_hook_guard::guard_channel_send`), so it is expected to be skipped
//! too — this file asserts that in
//! `init_in_a_tempdir_without_rite_data_dir_refuses_hook_and_skips_projects_send`.
//! The preliminary skeleton files are the only expected residue in the sentinel
//! directory; what's asserted is the guard's actual job: no hook lands
//! there, no message lands on `#projects`, `edict init` exits non-zero, and
//! the failure message names `RITE_DATA_DIR`.
//!
//! `allow_live_hooks_flag_lets_a_tempdir_project_register_everywhere` proves
//! the guard is non-vacuous the other way: with `--allow-live-hooks`, the
//! same tempdir project registers both the hook and the `#projects` message
//! in the sentinel data dir.

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;

/// Report whether the `rite` binary is on `PATH`.
///
/// Without a real `rite`, `edict init` never reaches the point of trying to
/// register a hook (it silently skips when `rite hooks list` fails), so the
/// test would be vacuous. Skip rather than fail, matching
/// `hermetic_rite_hooks.rs`.
fn rite_available() -> bool {
    Command::new("rite")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Env vars pointed at the sentinel `HOME`, with `RITE_DATA_DIR` explicitly
/// absent — the condition under test.
struct SentinelEnv {
    home: std::path::PathBuf,
}

impl SentinelEnv {
    fn apply<'a>(&self, cmd: &'a mut AssertCommand) -> &'a mut AssertCommand {
        cmd.env_remove("RITE_DATA_DIR")
            .env_remove("AGENT")
            .env_remove("RITE_AGENT")
            .env_remove("BOTBUS_AGENT")
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }

    fn apply_std<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env_remove("RITE_DATA_DIR")
            .env_remove("AGENT")
            .env_remove("RITE_AGENT")
            .env_remove("BOTBUS_AGENT")
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }
}

/// Read `rite history <channel> --format json` under `sentinel` and return
/// the `messages` array's length (`0` when the channel does not exist yet —
/// `rite history` on a channel with no messages still exits `0`).
fn message_count(sentinel: &SentinelEnv, channel: &str) -> usize {
    let mut cmd = Command::new("rite");
    sentinel.apply_std(&mut cmd);
    cmd.args(["history", channel, "--format", "json"]);
    let output = cmd.output().expect("run rite history");
    assert!(
        output.status.success(),
        "rite history {channel} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("rite history should print JSON");
    parsed["messages"]
        .as_array()
        .expect("rite history JSON should have a messages array")
        .len()
}

#[test]
fn init_in_a_tempdir_without_rite_data_dir_refuses_hook_and_skips_projects_send() {
    if !rite_available() {
        println!(
            "skipping init_in_a_tempdir_without_rite_data_dir_refuses_hook_and_skips_projects_send: \
             `rite` not found on PATH"
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir");
    let project_dir = tmp.path().join("project");
    let sentinel = SentinelEnv {
        home: tmp.path().join("home"),
    };
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    std::fs::create_dir_all(&sentinel.home).expect("create sentinel home dir");

    // A throwaway git repo, since `edict init` expects to run inside one.
    run_git(&project_dir, &["init", "-q"]);
    run_git(
        &project_dir,
        &["config", "user.email", "hermetic@example.com"],
    );
    run_git(&project_dir, &["config", "user.name", "Hermetic Test"]);
    run_git(
        &project_dir,
        &["commit", "--allow-empty", "-q", "-m", "initial commit"],
    );

    let project_name = "temp-guard-proj";

    let mut init_cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
    sentinel.apply(&mut init_cmd);
    init_cmd
        .current_dir(&project_dir)
        .arg("init")
        .arg("--no-interactive")
        .arg("--name")
        .arg(project_name)
        .arg("--type")
        .arg("cli")
        .arg("--tools")
        .arg("rite")
        .arg("--no-commit");

    let output = init_cmd.output().expect("run edict init");

    assert!(
        !output.status.success(),
        "edict init should have exited non-zero when the live-hook guard refused \
         registration, but it exited successfully.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("RITE_DATA_DIR"),
        "expected the refusal message to name RITE_DATA_DIR, got:\n{combined}"
    );
    assert!(
        combined.contains("--allow-live-hooks"),
        "expected the refusal message to mention --allow-live-hooks, got:\n{combined}"
    );

    // --- Assert: no hook landed in the sentinel's rite data ---
    let mut list_cmd = Command::new("rite");
    sentinel.apply_std(&mut list_cmd);
    list_cmd.args(["hooks", "list", "--format", "json"]);
    let list_output = list_cmd.output().expect("run rite hooks list");
    assert!(
        list_output.status.success(),
        "rite hooks list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("rite hooks list should print JSON");
    let hooks = parsed["hooks"]
        .as_array()
        .expect("rite hooks list JSON should have a hooks array");

    assert!(
        hooks.is_empty(),
        "expected no rite hooks under the sentinel HOME — the live-hook guard should have \
         refused registration before any `rite hooks add` ran — found: {stdout}"
    );

    // --- Assert: no message landed on #projects either ---
    let projects_messages = message_count(&sentinel, "projects");
    assert_eq!(
        projects_messages, 0,
        "expected no messages on #projects under the sentinel HOME — the same guard that \
         refuses the hook should have skipped the #projects registration send too"
    );
}

/// The other half of the guard: with `--allow-live-hooks`, a project rooted
/// under the system temp directory registers both the router hook *and*
/// the `#projects` message in the sentinel data dir. This proves the guard
/// (and its `#projects`-send counterpart) actually gate something, rather
/// than the previous test passing vacuously because nothing ever tries to
/// register in the first place.
#[test]
fn allow_live_hooks_flag_lets_a_tempdir_project_register_everywhere() {
    if !rite_available() {
        println!(
            "skipping allow_live_hooks_flag_lets_a_tempdir_project_register_everywhere: \
             `rite` not found on PATH"
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir");
    let project_dir = tmp.path().join("project");
    let sentinel = SentinelEnv {
        home: tmp.path().join("home"),
    };
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    std::fs::create_dir_all(&sentinel.home).expect("create sentinel home dir");

    run_git(&project_dir, &["init", "-q"]);
    run_git(
        &project_dir,
        &["config", "user.email", "hermetic@example.com"],
    );
    run_git(&project_dir, &["config", "user.name", "Hermetic Test"]);
    run_git(
        &project_dir,
        &["commit", "--allow-empty", "-q", "-m", "initial commit"],
    );

    let project_name = "temp-guard-proj-allowed";

    let mut init_cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
    sentinel.apply(&mut init_cmd);
    init_cmd
        .current_dir(&project_dir)
        .arg("init")
        .arg("--no-interactive")
        .arg("--name")
        .arg(project_name)
        .arg("--type")
        .arg("cli")
        .arg("--tools")
        .arg("rite")
        .arg("--no-commit")
        .arg("--allow-live-hooks");

    let output = init_cmd.output().expect("run edict init");

    assert!(
        output.status.success(),
        "edict init --allow-live-hooks should have succeeded for a tempdir project.\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // --- Assert: the router hook landed in the sentinel's rite data ---
    let mut list_cmd = Command::new("rite");
    sentinel.apply_std(&mut list_cmd);
    list_cmd.args(["hooks", "list", "--format", "json"]);
    let list_output = list_cmd.output().expect("run rite hooks list");
    assert!(
        list_output.status.success(),
        "rite hooks list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("rite hooks list should print JSON");
    let hooks = parsed["hooks"]
        .as_array()
        .expect("rite hooks list JSON should have a hooks array");
    assert!(
        !hooks.is_empty(),
        "expected --allow-live-hooks to let the router hook register in the sentinel HOME, \
         found none: {stdout}"
    );

    // --- Assert: the #projects registration message landed too ---
    let projects_messages = message_count(&sentinel, "projects");
    assert!(
        projects_messages > 0,
        "expected --allow-live-hooks to let the #projects registration send land in the \
         sentinel HOME, found {projects_messages} messages"
    );
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}
