//! `edict init --dry-run` in a project rooted under the system temp
//! directory, with `RITE_DATA_DIR` unset, must NOT abort: the live-hook
//! guard (`src/rite_hook_guard.rs`) records a planned refusal instead and
//! lets the rest of the plan render normally. A dry-run previews; it never
//! fails just because something it previewed would itself have failed.
//!
//! Contrast with `rite_hook_guard_refuses_live_hooks.rs`, which covers the
//! same guard in apply mode (`edict init` with no `--dry-run`), where a
//! refusal *does* abort with a non-zero exit.
//!
//! Like that test, `RITE_DATA_DIR` is left unset on purpose and `HOME`/
//! `XDG_DATA_HOME` point at a sentinel tempdir — rite's default data dir
//! derives from `HOME`/`XDG_DATA_HOME` (see
//! `rite/src/core/project.rs::data_dir()`), so if the guard were ever
//! silently broken, the live hook it would then actually register lands in
//! this disposable sentinel, never the real one — and this test can check,
//! positively, that no hook landed there.

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;

mod common;

/// Report whether the `rite` binary is on `PATH`.
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
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("VESSEL_SOCKET", common::vessel_socket_path(&self.home))
    }

    fn apply_std<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env_remove("RITE_DATA_DIR")
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("VESSEL_SOCKET", common::vessel_socket_path(&self.home))
    }
}

#[test]
fn init_dry_run_in_a_tempdir_without_rite_data_dir_plans_a_refusal_instead_of_aborting() {
    if !rite_available() {
        println!(
            "skipping init_dry_run_in_a_tempdir_without_rite_data_dir_plans_a_refusal_instead_of_aborting: \
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

    let project_name = "temp-guard-dry-run-proj";

    let mut init_cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
    sentinel.apply(&mut init_cmd);
    init_cmd
        .current_dir(&project_dir)
        .arg("init")
        .arg("--dry-run")
        .arg("--no-interactive")
        .arg("--name")
        .arg(project_name)
        .arg("--type")
        .arg("cli")
        .arg("--tools")
        .arg("rite")
        .arg("--no-commit");

    let output = init_cmd.output().expect("run edict init --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // A dry-run previews; it must finish successfully even though one of the
    // things it previewed (registering the hook) would itself be refused.
    assert!(
        output.status.success(),
        "edict init --dry-run should exit 0 even when the live-hook guard would refuse \
         registration, but it exited non-zero.\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The refusal shows up in the rendered plan (not as an abort).
    assert!(
        stdout.contains("would be REFUSED"),
        "expected the dry-run plan to note the refusal, got stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("RITE_DATA_DIR"),
        "expected the refusal note to name RITE_DATA_DIR, got stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("--allow-live-hooks"),
        "expected the refusal note to mention --allow-live-hooks, got stdout:\n{stdout}"
    );
    // The plan still finishes and renders normally — not an early abort.
    assert!(
        stdout.contains("Run without --dry-run to execute."),
        "expected the dry-run to render its full plan, got stdout:\n{stdout}"
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

    let hooks_stdout = String::from_utf8_lossy(&list_output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&hooks_stdout).expect("rite hooks list should print JSON");
    let hooks = parsed["hooks"]
        .as_array()
        .expect("rite hooks list JSON should have a hooks array");

    assert!(
        hooks.is_empty(),
        "expected no rite hooks under the sentinel HOME — a dry-run must never register \
         anything, guard refusal or not — found: {hooks_stdout}"
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
