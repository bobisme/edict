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
//! `src/commands/init.rs`) and its preliminary `rite hooks list` calls
//! create rite's on-disk skeleton (state files) as a side effect — both
//! unrelated to hook registration and outside this guard's scope. So the
//! sentinel directory is *not* expected to stay completely empty; what's
//! asserted is the guard's actual job: no hook lands there, `edict init`
//! exits non-zero, and the failure message names `RITE_DATA_DIR`.

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
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }

    fn apply_std<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env_remove("RITE_DATA_DIR")
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }
}

#[test]
fn init_in_a_tempdir_without_rite_data_dir_refuses_to_register_a_live_hook() {
    if !rite_available() {
        println!(
            "skipping init_in_a_tempdir_without_rite_data_dir_refuses_to_register_a_live_hook: \
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
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}
