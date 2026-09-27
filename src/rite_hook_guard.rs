//! Announcement and safety guard for `ensure_rite_hook`.
//!
//! `ensure_rite_hook` (see `crate::subprocess`) spawns `rite hooks add`,
//! which registers a hook that will later spawn a *real* AI agent whenever
//! its trigger fires. Two safety nets sit in front of that mutation:
//!
//! 1. [`announce`] prints, once per process, the rite data directory the
//!    hook is about to land in, followed by a line per hook describing what
//!    is being registered (name/channel/cwd/command). This makes a live
//!    mutation visible even when it happens deep inside `edict init`/`sync`.
//! 2. [`guard`] refuses to register a hook for a project whose root looks
//!    like a throwaway (under the system temp directory) when
//!    `RITE_DATA_DIR` is not set — the situation a hermetic test or a
//!    scratch project could otherwise leak a live hook from, straight into
//!    the machine's real rite data directory. `--allow-live-hooks` opts out
//!    explicitly.
//!
//! Both take an [`Effects`] so they behave correctly under `--dry-run`:
//! apply mode writes announcements to stderr and a refusal aborts with
//! `Err`; record mode instead feeds `fx`, so the data-dir line, the hook
//! line, and — on refusal — a "would be REFUSED" note all show up in the
//! rendered plan, and the dry-run finishes rather than aborting.
//!
//! [`guard_channel_send`] applies the same [`decide`] rule to a different
//! live mutation: `edict init`'s `rite send ... projects ...`, which
//! registers the project on the shared `#projects` channel. Unlike
//! [`guard`], a refusal there never fails `init` — it only skips that one
//! announcement.

use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::effects::Effects;

/// Environment variable rite reads to redirect its data directory.
///
/// Mirrored here rather than imported: edict shells out to the `rite`
/// binary and does not link against it as a library.
pub const RITE_DATA_DIR_ENV_VAR: &str = "RITE_DATA_DIR";

static ANNOUNCE_DATA_DIR_ONCE: Once = Once::new();

/// Resolve the rite data directory the same way rite itself does (see
/// `rite/src/core/project.rs::data_dir()`):
///
/// 1. `RITE_DATA_DIR`, if set and non-empty
/// 2. `$XDG_DATA_HOME/rite`, if `XDG_DATA_HOME` is set and non-empty
/// 3. `$HOME/.local/share/rite`
#[must_use]
pub fn resolve_rite_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var(RITE_DATA_DIR_ENV_VAR)
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("rite");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".local").join("share").join("rite");
    }
    PathBuf::from(".rite")
}

fn rite_data_dir_is_set() -> bool {
    std::env::var(RITE_DATA_DIR_ENV_VAR).is_ok_and(|v| !v.is_empty())
}

/// Pull the value of a `--flag value` pair out of a `rite hooks add` args
/// slice, returning the first match (the hook-level flag, not one that may
/// reappear inside the trailing spawned command).
fn extract_flag<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1])
}

/// The trailing command a hook runs, i.e. everything after the first `--`.
fn extract_command<'a>(args: &'a [&'a str]) -> &'a [&'a str] {
    args.iter()
        .position(|&a| a == "--")
        .map_or(&[], |i| &args[i + 1..])
}

/// Print the resolved rite data dir (once per process) and this hook's
/// name/channel/cwd/command.
///
/// Apply mode writes to stderr, so it's visible even when stdout is piped or
/// parsed. Record mode (`--dry-run`) instead feeds `fx.announce`, so the same
/// lines appear in the rendered plan rather than being lost on stderr.
pub fn announce(fx: &Effects, description: &str, add_args: &[&str]) {
    ANNOUNCE_DATA_DIR_ONCE.call_once(|| {
        let line = format!("rite data dir: {}", resolve_rite_data_dir().display());
        if fx.is_recording() {
            fx.announce(line);
        } else {
            eprintln!("{line}");
        }
    });
    let line = format!(
        "rite hook: name={:?} channel={:?} cwd={:?} command={:?}",
        description,
        extract_flag(add_args, "--channel").unwrap_or("<unknown>"),
        extract_flag(add_args, "--cwd").unwrap_or("<unknown>"),
        extract_command(add_args)
    );
    if fx.is_recording() {
        fx.announce(line);
    } else {
        eprintln!("{line}");
    }
}

/// A live rite hook registration was refused because the project looks like
/// a throwaway and no sandbox (`RITE_DATA_DIR`) was configured.
#[derive(Debug, thiserror::Error)]
#[error(
    "refusing to register a live rite hook: project root {cwd} is under the system temp \
     directory and RITE_DATA_DIR is not set, so this would register against the machine's \
     real rite data directory ({data_dir}). Set RITE_DATA_DIR to a sandbox directory, pass \
     --allow-live-hooks to register anyway, or use --dry-run to preview without registering."
)]
pub struct LiveHookRefused {
    cwd: String,
    data_dir: String,
}

/// Whether a live hook registration should be allowed or refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Refuse,
}

/// Decide whether registering a live rite hook for a project rooted at
/// `cwd` should be allowed.
///
/// Pure and side-effect free: takes the project cwd, the temp-dir roots to
/// check it against, whether `RITE_DATA_DIR` is set, and whether
/// `--allow-live-hooks` was passed. `cwd` and `temp_roots` should already be
/// resolved to a comparable form (e.g. both canonicalized) by the caller;
/// this function only compares them.
#[must_use]
pub fn decide(
    cwd: &Path,
    temp_roots: &[PathBuf],
    rite_data_dir_is_set: bool,
    allow_live_hooks: bool,
) -> Decision {
    if allow_live_hooks || rite_data_dir_is_set {
        return Decision::Allow;
    }
    if temp_roots.iter().any(|root| cwd.starts_with(root)) {
        Decision::Refuse
    } else {
        Decision::Allow
    }
}

/// The system temp-dir roots the guard checks a project's cwd against:
/// `std::env::temp_dir()` and `/tmp`, each canonicalized when possible.
///
/// Kept as two entries because `std::env::temp_dir()` does not always
/// resolve to `/tmp` (e.g. a `$TMPDIR` override, or a sandboxed environment
/// where the two diverge).
#[must_use]
pub fn default_temp_roots() -> Vec<PathBuf> {
    let env_temp = std::env::temp_dir();
    let slash_tmp = PathBuf::from("/tmp");
    vec![
        env_temp.canonicalize().unwrap_or(env_temp),
        slash_tmp.canonicalize().unwrap_or(slash_tmp),
    ]
}

/// Guard entry point: decide whether a live rite hook whose `rite hooks add`
/// cwd is embedded in `add_args` may be registered.
///
/// Honors dry-run semantics: in record mode (`fx.is_recording()`) a refusal
/// does not abort — a preview should always finish and show what it found,
/// including what it would have refused. Instead it's recorded via
/// `fx.announce` and the caller is told to skip the add (`Ok(false)`). In
/// apply mode a refusal aborts with `Err(LiveHookRefused)`.
///
/// Returns `Ok(true)` when registration should proceed, `Ok(false)` when it
/// was refused but recorded (dry-run only; the caller must skip the add).
///
/// # Errors
///
/// Returns `Err(LiveHookRefused)` in apply mode when the project looks like
/// a throwaway (its cwd is under the system temp dir) and neither
/// `RITE_DATA_DIR` nor `--allow-live-hooks` was given.
pub fn guard(
    fx: &Effects,
    description: &str,
    add_args: &[&str],
    allow_live_hooks: bool,
) -> anyhow::Result<bool> {
    let Some(cwd_str) = extract_flag(add_args, "--cwd") else {
        // No --cwd to check against; nothing to guard.
        return Ok(true);
    };
    let cwd = Path::new(cwd_str);
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let roots = default_temp_roots();

    match decide(&canon_cwd, &roots, rite_data_dir_is_set(), allow_live_hooks) {
        Decision::Allow => Ok(true),
        Decision::Refuse if fx.is_recording() => {
            fx.announce(format!(
                "hook {description} would be REFUSED: project is under a temp dir and \
                 RITE_DATA_DIR is unset (pass --allow-live-hooks or set RITE_DATA_DIR)"
            ));
            Ok(false)
        }
        Decision::Refuse => Err(LiveHookRefused {
            cwd: cwd_str.to_string(),
            data_dir: resolve_rite_data_dir().display().to_string(),
        }
        .into()),
    }
}

/// Guard a live-data mutation that is not a hook registration.
///
/// Specifically, this covers `edict init`'s `rite send ... projects ...`,
/// which announces a new project on the shared `#projects` channel (see
/// `register_project_channel` in `src/commands/init.rs`). That send writes
/// into the same live rite data directory `guard` protects, so a throwaway
/// project rooted under the system temp directory must not post to it
/// either.
///
/// Reuses [`decide`] with the project's own `cwd` (rather than one pulled
/// out of `rite hooks add` args), so this applies the identical rule as the
/// hook guard: refuse when `cwd` is under a system temp root and neither
/// `RITE_DATA_DIR` nor `--allow-live-hooks` was given.
///
/// Unlike [`guard`], a refusal here never fails the command — `init` already
/// fails apply-mode on a refused hook registration, and this send is a
/// secondary announcement, not something worth aborting init over. So this
/// returns a plain `bool` rather than `anyhow::Result<bool>`: `true` when
/// the send should proceed, `false` when it was refused. Apply mode prints a
/// clear skip message to stderr naming `RITE_DATA_DIR`/`--allow-live-hooks`;
/// record mode (`--dry-run`) instead records a "would be skipped" note via
/// `fx.announce`, so it shows up in the rendered plan.
#[must_use]
pub fn guard_channel_send(
    fx: &Effects,
    description: &str,
    cwd: &Path,
    allow_live_hooks: bool,
) -> bool {
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let roots = default_temp_roots();
    let decision = decide(&canon_cwd, &roots, rite_data_dir_is_set(), allow_live_hooks);
    channel_send_outcome(decision, fx, description)
}

/// The decision-to-outcome half of [`guard_channel_send`], factored out so
/// it can be unit tested directly against a given [`Decision`] rather than
/// through the process-global state (`RITE_DATA_DIR`, `std::env::temp_dir()`)
/// that [`guard_channel_send`] resolves that decision from.
fn channel_send_outcome(decision: Decision, fx: &Effects, description: &str) -> bool {
    match decision {
        Decision::Allow => true,
        Decision::Refuse if fx.is_recording() => {
            fx.announce(format!(
                "{description} would be skipped: project is under a temp dir and \
                 RITE_DATA_DIR is unset (pass --allow-live-hooks or set RITE_DATA_DIR)"
            ));
            false
        }
        Decision::Refuse => {
            eprintln!(
                "Skipping {description}: project is under a temp dir and RITE_DATA_DIR is \
                 unset (pass --allow-live-hooks to send anyway, or set RITE_DATA_DIR to a \
                 sandbox directory)"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn allows_non_temp_project() {
        let roots = vec![p("/tmp")];
        assert_eq!(
            decide(&p("/home/bob/src/edict"), &roots, false, false),
            Decision::Allow
        );
    }

    #[test]
    fn refuses_temp_project_without_sandbox_or_flag() {
        let roots = vec![p("/tmp")];
        assert_eq!(
            decide(&p("/tmp/some-scratch-project"), &roots, false, false),
            Decision::Refuse
        );
    }

    #[test]
    fn allows_temp_project_when_rite_data_dir_is_set() {
        let roots = vec![p("/tmp")];
        assert_eq!(
            decide(&p("/tmp/some-scratch-project"), &roots, true, false),
            Decision::Allow
        );
    }

    #[test]
    fn allows_temp_project_with_explicit_flag() {
        let roots = vec![p("/tmp")];
        assert_eq!(
            decide(&p("/tmp/some-scratch-project"), &roots, false, true),
            Decision::Allow
        );
    }

    #[test]
    fn checks_all_configured_temp_roots() {
        let roots = vec![p("/tmp"), p("/private/var/folders")];
        assert_eq!(
            decide(&p("/private/var/folders/xy/scratch"), &roots, false, false),
            Decision::Refuse
        );
    }

    #[test]
    fn a_path_that_merely_contains_tmp_as_a_substring_is_not_under_it() {
        // "/tmpfoo/project" is not under "/tmp" — starts_with is
        // component-aware, not a string prefix check.
        let roots = vec![p("/tmp")];
        assert_eq!(
            decide(&p("/tmpfoo/project"), &roots, false, false),
            Decision::Allow
        );
    }

    #[test]
    fn extract_flag_finds_cwd() {
        let args = ["--agent", "a", "--cwd", "/x/y", "--ttl", "600"];
        assert_eq!(extract_flag(&args, "--cwd"), Some("/x/y"));
    }

    #[test]
    fn extract_command_finds_trailing_args() {
        let args = ["--cwd", "/x", "--", "vessel", "spawn"];
        assert_eq!(extract_command(&args), &["vessel", "spawn"]);
    }

    #[test]
    fn extract_command_empty_without_separator() {
        let args = ["--cwd", "/x"];
        assert_eq!(extract_command(&args), &[] as &[&str]);
    }

    #[test]
    fn channel_send_allows_when_decision_is_allow() {
        let fx = crate::effects::Effects::record();
        assert!(channel_send_outcome(
            Decision::Allow,
            &fx,
            "project registration"
        ));
        assert!(fx.planned().is_empty());
    }

    #[test]
    fn channel_send_records_would_be_skipped_note_when_refused_in_record_mode() {
        let fx = crate::effects::Effects::record();
        assert!(!channel_send_outcome(
            Decision::Refuse,
            &fx,
            "project registration"
        ));
        let planned = fx.planned();
        assert!(
            planned.iter().any(|item| matches!(
                item,
                crate::effects::Planned::Step(s)
                    if s.contains("would be skipped") && s.contains("RITE_DATA_DIR")
            )),
            "expected a skip note mentioning RITE_DATA_DIR, got: {planned:?}"
        );
    }

    #[test]
    fn channel_send_refuses_in_apply_mode_without_panicking() {
        let fx = crate::effects::Effects::apply();
        assert!(!channel_send_outcome(
            Decision::Refuse,
            &fx,
            "project registration"
        ));
    }
}
