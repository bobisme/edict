//! `edict sync --dry-run` must report what a sync would change and change
//! nothing: not the project tree, not the rite data dir, not `$HOME`.
//!
//! Runs against a sandbox like `hermetic_rite_hooks.rs`: `RITE_DATA_DIR` and a
//! sentinel `HOME` (plus XDG dirs) all inside one tempdir, so neither the
//! setup nor the command under test can reach the machine's real rite hooks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use sha2::{Digest, Sha256};

mod common;
// rite records a heartbeat for the ambient agent identity on *every*
// command, read-only ones included. That is rite's bookkeeping, not a change
// edict makes, so the sandbox runs with no agent identity: then the only way
// the rite data dir can change is a mutating `rite` call.
use common::AGENT_IDENTITY_VARS as AGENT_VARS;

fn rite_available() -> bool {
    Command::new("rite")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Sandbox {
    root: PathBuf,
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

    fn rite(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("rite");
        for (k, v) in self.envs() {
            cmd.env(k, v);
        }
        for k in AGENT_VARS {
            cmd.env_remove(k);
        }
        cmd.args(args).output().expect("run rite")
    }

    fn hooks(&self) -> Vec<serde_json::Value> {
        let out = self.rite(&["hooks", "list", "--format", "json"]);
        assert!(out.status.success(), "rite hooks list failed");
        let parsed: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("rite hooks list JSON");
        parsed["hooks"].as_array().cloned().unwrap_or_default()
    }
}

/// Every path under `root` (files, dirs, symlinks) with a hash of its content.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("dir entry").path();
            let rel = path.strip_prefix(root).expect("prefix").to_path_buf();
            let meta = std::fs::symlink_metadata(&path).expect("metadata");
            let digest = if meta.file_type().is_symlink() {
                format!(
                    "link:{}",
                    std::fs::read_link(&path).expect("readlink").display()
                )
            } else if meta.is_dir() {
                stack.push(path);
                "dir".to_string()
            } else {
                let bytes = std::fs::read(&path).expect("read file");
                format!("{:x}", Sha256::digest(&bytes))
            };
            out.insert(rel, digest);
        }
    }
    out
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

#[test]
fn sync_dry_run_lists_pending_changes_and_changes_nothing() {
    if !rite_available() {
        println!("skipping sync_dry_run: `rite` not found on PATH");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let sb = Sandbox {
        root: tmp.path().to_path_buf(),
        project: tmp.path().join("project"),
        rite_data_dir: tmp.path().join("rite"),
        home: tmp.path().join("home"),
    };
    std::fs::create_dir_all(&sb.project).expect("mkdir project");
    std::fs::create_dir_all(&sb.home).expect("mkdir home");
    run_git(&sb.project, &["init", "-q"]);
    run_git(
        &sb.project,
        &["config", "user.email", "dry-run@example.com"],
    );
    run_git(&sb.project, &["config", "user.name", "Dry Run Test"]);
    run_git(
        &sb.project,
        &["commit", "--allow-empty", "-q", "-m", "init"],
    );

    let name = "dry-run-proj";
    let init = sb.edict(&[
        "init",
        "--no-interactive",
        "--name",
        name,
        "--type",
        "cli",
        "--tools",
        "rite",
        "--no-commit",
    ]);
    assert_ok(&init, "edict init");
    run_git(&sb.project, &["add", "-A"]);
    run_git(&sb.project, &["commit", "-q", "-m", "edict init"]);

    // Make the project stale: a managed doc deleted and its version marker
    // out of date, and the router hook gone from the (sandboxed) store.
    let agents = sb.project.join(".agents/edict");
    std::fs::remove_file(agents.join("triage.md")).expect("delete triage.md");
    std::fs::write(agents.join(".version"), "stale\n").expect("stale .version");
    let description = format!("edict:{name}:responder");
    let hook_ids: Vec<String> = sb
        .hooks()
        .iter()
        .filter(|h| h["description"].as_str() == Some(description.as_str()))
        .filter_map(|h| h["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !hook_ids.is_empty(),
        "init should have registered the router hook"
    );
    for id in &hook_ids {
        assert_ok(&sb.rite(&["hooks", "remove", id]), "rite hooks remove");
    }

    let before = snapshot(&sb.root);
    let dry = sb.edict(&["sync", "--dry-run"]);
    let after = snapshot(&sb.root);
    assert_ok(&dry, "edict sync --dry-run");
    let stdout = String::from_utf8_lossy(&dry.stdout);

    assert_eq!(
        before, after,
        "sync --dry-run changed files (project, rite data dir or HOME).\nstdout: {stdout}"
    );

    // The plan names each pending change.
    for expected in [
        "Would change:",
        "create   .agents/edict/triage.md",
        "update   .agents/edict/.version",
        &format!("hook     {description} (create)"),
        &format!("channel: {name}"),
        &format!(
            "cwd:     {}",
            sb.project.canonicalize().expect("canon").display()
        ),
        "command: vessel spawn",
        "edict run responder",
        "run      git commit -m chore: edict sync",
        "Run without --dry-run to execute.",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
    assert!(!stdout.contains("Sync complete"), "{stdout}");

    // The plan was real: an actual sync performs it.
    let real = sb.edict(&["sync", "--no-commit"]);
    assert_ok(&real, "edict sync");
    assert!(agents.join("triage.md").exists());
    assert!(
        sb.hooks()
            .iter()
            .any(|h| h["description"].as_str() == Some(description.as_str())),
        "real sync should re-register the hook the dry-run planned"
    );
}
