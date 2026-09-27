//! Hermetic test: `edict init` and `edict sync` must register rite hooks
//! *only* inside `RITE_DATA_DIR`.
//!
//! rite resolves its data directory from `RITE_DATA_DIR` first (see
//! `rite/src/core/project.rs::data_dir()`), falling back to XDG dirs and then
//! `$HOME/.local/share/rite`. edict spawns `rite` (and `bn`, `maw`, `seal`,
//! `vessel`) as subprocesses via `crate::subprocess::Tool`, which never calls
//! `env_clear`/`env_remove` and never overrides `RITE_DATA_DIR`/`HOME`, so a
//! child process should always see whatever the parent `edict` process saw.
//!
//! This test proves that end to end: it points `RITE_DATA_DIR` at a tempdir
//! and `HOME` (plus the XDG dirs) at a *different*, sentinel tempdir, runs
//! `edict init` then `edict sync` against a throwaway project, and asserts
//! that the router hook rite registers landed in `RITE_DATA_DIR` — and that
//! nothing was ever created under the sentinel `HOME`'s default rite
//! location (`~/.local/share/rite`), which is where a dropped `RITE_DATA_DIR`
//! would leak to.
//!
//! It talks only to a sandboxed `RITE_DATA_DIR`; it never touches the
//! machine's real rite data directory.

use std::path::Path;
use std::process::Command;

use assert_cmd::Command as AssertCommand;

/// Report whether the `rite` binary is on `PATH`.
///
/// The hermetic hook-registration proof is meaningless without a real
/// `rite` to register hooks with, so the test skips (rather than fails)
/// when it's absent — e.g. a minimal CI image that doesn't ship the
/// companion tools.
fn rite_available() -> bool {
    Command::new("rite")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The env vars a hermetic edict invocation needs, all pointed at the
/// sandbox. `RITE_DATA_DIR` is the one under test; `HOME` and the `XDG_*`
/// vars are a sentinel — if `RITE_DATA_DIR` were ever dropped by a
/// subprocess hop, rite would fall back to one of these, and the test's
/// "nothing under the sentinel" assertion would catch it.
struct SandboxEnv {
    rite_data_dir: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl SandboxEnv {
    fn apply<'a>(&self, cmd: &'a mut AssertCommand) -> &'a mut AssertCommand {
        cmd.env("RITE_DATA_DIR", &self.rite_data_dir)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }

    fn apply_std<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("RITE_DATA_DIR", &self.rite_data_dir)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
    }

    /// The location a dropped `RITE_DATA_DIR` would fall back to: the
    /// sentinel `HOME`'s own `~/.local/share/rite`.
    fn sentinel_leak_path(&self) -> std::path::PathBuf {
        self.home.join(".local/share/rite")
    }
}

#[test]
fn init_and_sync_register_rite_hooks_only_in_rite_data_dir() {
    if !rite_available() {
        println!(
            "skipping init_and_sync_register_rite_hooks_only_in_rite_data_dir: \
             `rite` not found on PATH"
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("create tempdir");
    let project_dir = tmp.path().join("project");
    let sandbox = SandboxEnv {
        rite_data_dir: tmp.path().join("rite"),
        home: tmp.path().join("home"),
    };
    std::fs::create_dir_all(&project_dir).expect("create project dir");

    // A throwaway git repo, since edict init/sync both expect to run inside
    // one and this keeps the scenario close to a real project.
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

    let project_name = "hermetic-rite-proj";

    // --- edict init ---
    //
    // Only `rite` is enabled: `bones`/`maw`/`seal`/`vessel` would pull in
    // more subprocesses and, for maw in particular, a whole bare-repo
    // layout that isn't needed to prove hook registration is hermetic. No
    // `--language` is passed either, since that triggers a network fetch
    // for a .gitignore template — irrelevant here and not hermetic.
    let mut init_cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
    sandbox.apply(&mut init_cmd);
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

    let init_output = init_cmd.output().expect("run edict init");
    assert!(
        init_output.status.success(),
        "edict init failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&init_output.stdout),
        String::from_utf8_lossy(&init_output.stderr)
    );

    assert!(
        project_dir.join(".edict.toml").exists(),
        "edict init should have written .edict.toml"
    );

    // --- edict sync ---
    let mut sync_cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
    sandbox.apply(&mut sync_cmd);
    sync_cmd
        .current_dir(&project_dir)
        .arg("sync")
        .arg("--no-commit");

    let sync_output = sync_cmd.output().expect("run edict sync");
    assert!(
        sync_output.status.success(),
        "edict sync failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&sync_output.stdout),
        String::from_utf8_lossy(&sync_output.stderr)
    );

    // --- Assert: the hook landed in the sandboxed RITE_DATA_DIR ---
    let mut list_cmd = Command::new("rite");
    sandbox.apply_std(&mut list_cmd);
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

    // Not vacuous: prove a hook for *this* project actually exists, not
    // just that the array is non-empty.
    let expected_description = format!("edict:{project_name}:responder");
    let matching = hooks
        .iter()
        .filter(|h| h["description"].as_str() == Some(expected_description.as_str()))
        .count();
    assert!(
        matching > 0,
        "expected a rite hook with description {expected_description:?} in the sandboxed \
         RITE_DATA_DIR, found hooks: {stdout}"
    );
    assert!(
        !hooks.is_empty(),
        "sandboxed RITE_DATA_DIR should contain at least one hook after init+sync"
    );

    // --- Assert: nothing leaked to the sentinel HOME's default rite location ---
    let leak_path = sandbox.sentinel_leak_path();
    assert!(
        !leak_path.exists(),
        "RITE_DATA_DIR was not honored by some subprocess: found rite data under the \
         sentinel HOME at {} (this is where a dropped RITE_DATA_DIR falls back to)",
        leak_path.display()
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
