//! Non-interactive `edict init --tools bones` must actually run `bn init`.
//!
//! `init_bones` used to default to `false` whenever there was no interactive
//! prompt to confirm it, so `edict init --no-interactive --tools bones` left
//! a project with no `.bones/` at all even though the `bones` tool was
//! explicitly enabled (bn-2qkz). This proves the non-interactive default now
//! matches the interactive prompt's own default (yes), that `--no-init-bones`
//! still opts out, and that the dry-run plan lists the `bn init` step.
//!
//! Sandbox like `init_dry_run.rs`: `RITE_DATA_DIR` and a sentinel `HOME`
//! (plus the XDG dirs and `VESSEL_SOCKET`) all inside one tempdir, so
//! nothing here can reach the machine's real rite/vessel/bones state.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;

mod common;
use common::AGENT_IDENTITY_VARS as AGENT_VARS;

/// Report whether the `bn` binary is on `PATH`.
///
/// Without a real `bn`, `edict init` warns and continues (see
/// `src/commands/init.rs`), so the test would be vacuous. Skip rather than
/// fail, matching the other hermetic tests' `rite_available` gates.
fn bn_available() -> bool {
    Command::new("bn")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Sandbox {
    project: PathBuf,
    rite_data_dir: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("RITE_DATA_DIR", self.rite_data_dir.clone()),
            ("HOME", self.home.clone()),
            ("XDG_DATA_HOME", self.home.join(".local/share")),
            ("XDG_CONFIG_HOME", self.home.join(".config")),
            ("XDG_CACHE_HOME", self.home.join(".cache")),
            ("XDG_STATE_HOME", self.home.join(".local/state")),
            ("VESSEL_SOCKET", common::vessel_socket_path(&self.home)),
        ]
    }

    fn edict(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = AssertCommand::cargo_bin("edict").expect("find edict binary");
        for (k, v) in self.envs() {
            cmd.env(k, v);
        }
        for k in AGENT_VARS {
            cmd.env_remove(k);
        }
        cmd.current_dir(&self.project)
            .args(args)
            .output()
            .expect("run edict")
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn assert_ok(out: &std::process::Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn new_sandbox() -> (tempfile::TempDir, Sandbox) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sb = Sandbox {
        project: tmp.path().join("project"),
        rite_data_dir: tmp.path().join("rite"),
        home: tmp.path().join("home"),
    };
    std::fs::create_dir_all(&sb.project).expect("mkdir project");
    std::fs::create_dir_all(&sb.home).expect("mkdir home");
    run_git(&sb.project, &["init", "-q"]);
    run_git(
        &sb.project,
        &["config", "user.email", "bones-default@example.com"],
    );
    run_git(&sb.project, &["config", "user.name", "Bones Default Test"]);
    run_git(
        &sb.project,
        &["commit", "--allow-empty", "-q", "-m", "init"],
    );
    (tmp, sb)
}

#[test]
fn non_interactive_init_with_bones_initializes_bones_by_default() {
    if !bn_available() {
        println!(
            "skipping non_interactive_init_with_bones_initializes_bones_by_default: \
             `bn` not found on PATH"
        );
        return;
    }
    let (_tmp, sb) = new_sandbox();

    let real = sb.edict(&[
        "init",
        "--no-interactive",
        "--name",
        "bones-default-proj",
        "--type",
        "cli",
        "--tools",
        "bones",
        "--no-commit",
    ]);
    assert_ok(&real, "edict init --tools bones --no-interactive");
    assert!(
        sb.project.join(".bones").is_dir(),
        "non-interactive init with the bones tool enabled should have run `bn init`"
    );
}

#[test]
fn no_init_bones_flag_still_opts_out() {
    if !bn_available() {
        println!("skipping no_init_bones_flag_still_opts_out: `bn` not found on PATH");
        return;
    }
    let (_tmp, sb) = new_sandbox();

    let real = sb.edict(&[
        "init",
        "--no-interactive",
        "--name",
        "bones-opt-out-proj",
        "--type",
        "cli",
        "--tools",
        "bones",
        "--no-init-bones",
        "--no-commit",
    ]);
    assert_ok(&real, "edict init --no-init-bones");
    assert!(
        !sb.project.join(".bones").exists(),
        "--no-init-bones should still skip `bn init`"
    );
}

#[test]
fn dry_run_plan_lists_bn_init() {
    if !bn_available() {
        println!("skipping dry_run_plan_lists_bn_init: `bn` not found on PATH");
        return;
    }
    let (_tmp, sb) = new_sandbox();

    let dry = sb.edict(&[
        "init",
        "--dry-run",
        "--no-interactive",
        "--name",
        "bones-dry-run-proj",
        "--type",
        "cli",
        "--tools",
        "bones",
        "--no-commit",
    ]);
    assert_ok(&dry, "edict init --dry-run --tools bones");
    let stdout = String::from_utf8_lossy(&dry.stdout);
    assert!(
        stdout.contains("run      bn init"),
        "expected the dry-run plan to list `bn init`, got:\n{stdout}"
    );
    assert!(
        !sb.project.join(".bones").exists(),
        "dry-run must not actually create .bones/"
    );
}
