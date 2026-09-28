//! `edict init --dry-run` must list every action init would take and perform
//! none of them: not in the project tree (`.git` included), not in the rite
//! data dir, not under `$HOME` (where init installs the global agent hooks).
//!
//! Runs against a sandbox like `sync_dry_run.rs`: `RITE_DATA_DIR` and a
//! sentinel `HOME` (plus XDG dirs) all inside one tempdir, so neither the
//! setup nor the command under test can reach the machine's real rite hooks.
//! No `--language`: a real init would fetch a `.gitignore` over the network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use sha2::{Digest, Sha256};

mod common;
// rite records a heartbeat for the ambient agent identity on *every*
// command, read-only ones included. The sandbox runs with no agent identity,
// so the only way the rite data dir can change is a mutating `rite` call.
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

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed in {}",
        dir.display()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
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
fn init_dry_run_lists_every_action_and_changes_nothing() {
    if !rite_available() {
        println!("skipping init_dry_run: `rite` not found on PATH");
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
    // rite lays out its data dir on first use, even for a read-only
    // `hooks list` (which init runs to plan the hook). Do that first so the
    // snapshot measures edict's changes, not rite's lazy setup.
    assert!(sb.hooks().is_empty(), "fresh sandbox has no hooks");

    let name = "dry-init-proj";
    let init_args = [
        "init",
        "--no-interactive",
        "--name",
        name,
        "--type",
        "cli",
        "--tools",
        "bones,maw,seal,rite,vessel",
    ];

    let before = snapshot(&sb.root);
    let mut dry_args = init_args.to_vec();
    dry_args.push("--dry-run");
    let dry = sb.edict(&dry_args);
    let after = snapshot(&sb.root);
    assert_ok(&dry, "edict init --dry-run");
    let stdout = String::from_utf8_lossy(&dry.stdout);

    assert_eq!(
        before, after,
        "init --dry-run changed files (project, .git, rite data dir or HOME).\nstdout: {stdout}"
    );
    assert!(sb.hooks().is_empty(), "dry-run registered a rite hook");

    let project = sb.project.canonicalize().expect("canon");
    let home = sb.home.canonicalize().expect("canon");
    let description = format!("edict:{name}:responder");
    for expected in [
        "Would change:".to_string(),
        // Project files.
        "mkdir    .agents/edict/".to_string(),
        "create   .edict.toml".to_string(),
        "create   .agents/edict/triage.md".to_string(),
        "create   .agents/edict/.version".to_string(),
        "create   .agents/edict/design/cli-conventions.md".to_string(),
        "create   AGENTS.md".to_string(),
        "create   .sealignore".to_string(),
        // Global agent hooks under $HOME.
        format!("create   {}", home.join(".claude/settings.json").display()),
        format!(
            "create   {}",
            home.join(".pi/agent/extensions/edict-hooks.ts").display()
        ),
        // Tool inits.
        format!("run      maw init  (in {})", project.display()),
        format!("run      seal init  (in {})", project.display()),
        "run      rite send --agent dry-init-proj-dev projects".to_string(),
        // The rite hook.
        format!("hook     {description} (create)"),
        format!("channel: {name}"),
        format!("cwd:     {}", project.display()),
        "command: vessel spawn".to_string(),
        "edict run responder".to_string(),
        // The commit.
        "run      git add -A".to_string(),
        "run      git commit -m chore: initialize edict v".to_string(),
        "Run without --dry-run to execute.".to_string(),
    ] {
        assert!(
            stdout.contains(&expected),
            "missing {expected:?} in:\n{stdout}"
        );
    }
    assert!(!stdout.contains("Done."), "{stdout}");

    // The plan was real: an actual init in the same sandbox performs it.
    let real = sb.edict(&init_args);
    assert_ok(&real, "edict init");
    assert!(sb.project.join(".agents/edict/triage.md").exists());
    assert!(sb.project.join("AGENTS.md").exists());
    assert!(sb.project.join(".edict.toml").exists());
    assert!(sb.home.join(".claude/settings.json").exists());
    assert!(
        sb.hooks()
            .iter()
            .any(|h| h["description"].as_str() == Some(description.as_str())),
        "real init should register the hook the dry-run planned"
    );
    assert!(
        run_git(&sb.project, &["log", "--oneline"]).contains("chore: initialize edict"),
        "real init should commit"
    );
}
