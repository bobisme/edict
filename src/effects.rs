//! The single choke point for side effects of project-mutating commands.
//!
//! `edict sync` and `edict init` change a project in two ways: they write
//! files and they run subprocesses that mutate state outside the tree —
//! `rite hooks add`, `git commit`, `bn init`. Every such change goes through an
//! [`Effects`] value passed down the call chain. It runs in one of two modes:
//!
//! - **apply** — perform each change, exactly as a direct `fs::write` or
//!   `Tool::run` would.
//! - **record** — perform nothing; append a [`Planned`] entry instead. This is
//!   what `--dry-run` uses. Callers never test for dry-run themselves.
//!
//! Reads that happen *after* a recorded write must see the planned content, or
//! a dry-run of a multi-step migration would plan from stale state. So in
//! record mode [`Effects`] keeps an overlay of the files it would have written,
//! and [`Effects::read_to_string`] / [`Effects::exists`] consult it first.
//!
//! Read-only subprocesses (`rite hooks list`, `--help` probes) are not effects:
//! call them directly with [`Tool::run`] — they run in both modes, so a
//! dry-run can decide between adopt, update and create exactly as a real run
//! would. A read-only probe whose answer would depend on an earlier recorded
//! mutation (e.g. "is anything staged?" after a recorded `git add`) goes
//! through [`Effects::probe`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::subprocess::{RunOutput, Tool, run_command};

/// One change that record mode captured instead of performing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// A file that does not exist yet would be written.
    CreateFile { path: PathBuf, lines: usize },
    /// An existing file would be rewritten with different content.
    UpdateFile {
        path: PathBuf,
        added: usize,
        removed: usize,
    },
    /// A file would be written with content produced at apply time (e.g.
    /// fetched over the network), which a dry-run does not produce.
    GenerateFile { path: PathBuf, source: String },
    /// A file would be deleted.
    RemoveFile { path: PathBuf },
    /// A directory (and, for `remove_dir_all`, its contents) would be deleted.
    RemoveDir { path: PathBuf },
    /// A directory would be created.
    CreateDir { path: PathBuf },
    /// A file or directory would be renamed.
    Rename { from: PathBuf, to: PathBuf },
    /// A symlink would be created (or replaced) at `link`, pointing at `target`.
    Symlink { link: PathBuf, target: String },
    /// A mutating subprocess would run.
    Run {
        program: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        /// Why it would run, when the caller knows more than the command line
        /// says (e.g. "create" vs "update in place" for `rite hooks add`).
        intent: Option<String>,
    },
    /// A step the command would announce (a migration, a cleanup).
    Step(String),
}

#[derive(Debug, Default)]
struct Recorder {
    planned: Vec<Planned>,
    /// Path → content it would have after the planned writes (`None`: removed).
    overlay: HashMap<PathBuf, Option<String>>,
}

fn push(recorder: &RefCell<Recorder>, item: Planned) {
    recorder.borrow_mut().planned.push(item);
}

/// Execution context: performs side effects, or records them for a dry-run.
#[derive(Debug)]
pub struct Effects {
    recorder: Option<RefCell<Recorder>>,
}

impl Effects {
    /// Perform every effect for real.
    #[must_use]
    pub const fn apply() -> Self {
        Self { recorder: None }
    }

    /// Record every effect without performing it (`--dry-run`).
    #[must_use]
    pub fn record() -> Self {
        Self {
            recorder: Some(RefCell::new(Recorder::default())),
        }
    }

    /// Whether this context records instead of performing.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// The changes recorded so far (always empty in apply mode).
    #[must_use]
    pub fn planned(&self) -> Vec<Planned> {
        self.recorder
            .as_ref()
            .map(|r| r.borrow().planned.clone())
            .unwrap_or_default()
    }

    /// Announce a step. Apply mode prints it; record mode lists it in the plan.
    pub fn announce(&self, message: impl Into<String>) {
        let message = message.into();
        match &self.recorder {
            None => println!("{message}"),
            Some(r) => push(r, Planned::Step(message)),
        }
    }

    /// Report the outcome of an effect already in the plan ("Generated
    /// AGENTS.md"). Apply mode prints it; record mode drops it, since the
    /// recorded effect already says the same thing.
    pub fn report(&self, message: impl AsRef<str>) {
        if self.recorder.is_none() {
            println!("{}", message.as_ref());
        }
    }

    /// Read a file, seeing any content a recorded write would have left.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the file cannot be read (or a recorded effect removed it).
    pub fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        if let Some(r) = &self.recorder
            && let Some(entry) = r.borrow().overlay.get(path)
        {
            return entry.clone().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{} would be removed", path.display()),
                )
            });
        }
        fs::read_to_string(path)
    }

    /// Whether a path exists, counting files recorded writes would create or remove.
    #[must_use]
    pub fn exists(&self, path: &Path) -> bool {
        if let Some(r) = &self.recorder
            && let Some(entry) = r.borrow().overlay.get(path)
        {
            return entry.is_some();
        }
        path.exists()
    }

    /// Write `contents` to `path`.
    ///
    /// Record mode lists the write only when it would change the file, with a
    /// line-level summary of the difference.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the write fails (apply mode only).
    pub fn write(&self, path: &Path, contents: impl AsRef<str>) -> Result<()> {
        let contents = contents.as_ref();
        let Some(r) = &self.recorder else {
            return fs::write(path, contents)
                .with_context(|| format!("Failed to write {}", path.display()));
        };
        let current = self.read_to_string(path).ok();
        let item = match &current {
            Some(old) if old == contents => None,
            Some(old) => {
                let (added, removed) = line_delta(old, contents);
                Some(Planned::UpdateFile {
                    path: path.to_path_buf(),
                    added,
                    removed,
                })
            }
            None => Some(Planned::CreateFile {
                path: path.to_path_buf(),
                lines: contents.lines().count(),
            }),
        };
        let mut rec = r.borrow_mut();
        rec.overlay
            .insert(path.to_path_buf(), Some(contents.to_string()));
        if let Some(item) = item {
            rec.planned.push(item);
        }
        Ok(())
    }

    /// Write `path` with content from `produce`, which may be expensive or
    /// reach outside the machine (a network fetch).
    ///
    /// Apply mode calls `produce` and writes its result. Record mode calls
    /// nothing and lists the write with `source` (where the content would
    /// come from). The overlay does not learn the content, so later
    /// [`Effects::read_to_string`] calls still see the file as it is on disk.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `produce` or the write fails (apply mode only).
    pub fn write_generated(
        &self,
        path: &Path,
        source: impl Into<String>,
        produce: impl FnOnce() -> Result<String>,
    ) -> Result<()> {
        let Some(r) = &self.recorder else {
            let contents = produce()?;
            return fs::write(path, contents)
                .with_context(|| format!("Failed to write {}", path.display()));
        };
        push(
            r,
            Planned::GenerateFile {
                path: path.to_path_buf(),
                source: source.into(),
            },
        );
        Ok(())
    }

    /// Delete a file.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the removal fails (apply mode only).
    pub fn remove_file(&self, path: &Path) -> Result<()> {
        let Some(r) = &self.recorder else {
            return fs::remove_file(path)
                .with_context(|| format!("Failed to remove {}", path.display()));
        };
        let mut rec = r.borrow_mut();
        rec.overlay.insert(path.to_path_buf(), None);
        rec.planned.push(Planned::RemoveFile {
            path: path.to_path_buf(),
        });
        Ok(())
    }

    /// Delete a directory and everything in it.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the removal fails (apply mode only).
    pub fn remove_dir_all(&self, path: &Path) -> Result<()> {
        let Some(r) = &self.recorder else {
            return fs::remove_dir_all(path)
                .with_context(|| format!("Failed to remove {}", path.display()));
        };
        push(
            r,
            Planned::RemoveDir {
                path: path.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Delete an empty directory.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the removal fails (apply mode only).
    pub fn remove_dir(&self, path: &Path) -> Result<()> {
        let Some(r) = &self.recorder else {
            return fs::remove_dir(path)
                .with_context(|| format!("Failed to remove {}", path.display()));
        };
        push(
            r,
            Planned::RemoveDir {
                path: path.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Create a directory and any missing parents.
    ///
    /// # Errors
    ///
    /// Returns `Err` if creation fails (apply mode only).
    pub fn create_dir_all(&self, path: &Path) -> Result<()> {
        let Some(r) = &self.recorder else {
            return fs::create_dir_all(path)
                .with_context(|| format!("Failed to create {}", path.display()));
        };
        if !path.is_dir() {
            push(
                r,
                Planned::CreateDir {
                    path: path.to_path_buf(),
                },
            );
        }
        Ok(())
    }

    /// Rename a file or directory.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the rename fails (apply mode only).
    pub fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let Some(r) = &self.recorder else {
            return fs::rename(from, to).with_context(|| {
                format!("Failed to rename {} to {}", from.display(), to.display())
            });
        };
        // Carry file content across so later reads of `to` see it.
        let moved = if from.is_file() {
            self.read_to_string(from).ok()
        } else {
            None
        };
        let mut rec = r.borrow_mut();
        if let Some(content) = moved {
            rec.overlay.insert(to.to_path_buf(), Some(content));
            rec.overlay.insert(from.to_path_buf(), None);
        }
        rec.planned.push(Planned::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        Ok(())
    }

    /// Point a symlink at `link` to `target`, replacing whatever is there
    /// atomically (temp link, then rename over).
    ///
    /// # Errors
    ///
    /// Returns `Err` if the link cannot be created (apply mode only).
    pub fn symlink_replace(&self, target: &str, link: &Path) -> Result<()> {
        if let Some(r) = &self.recorder {
            push(
                r,
                Planned::Symlink {
                    link: link.to_path_buf(),
                    target: target.to_string(),
                },
            );
            return Ok(());
        }
        let mut tmp_name = link.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp_link = link.with_file_name(tmp_name);
        let _ = fs::remove_file(&tmp_link); // clean up any stale temp
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, &tmp_link)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, &tmp_link)?;
        if let Err(e) = fs::rename(&tmp_link, link) {
            let _ = fs::remove_file(&tmp_link);
            return Err(e).with_context(|| format!("creating {} symlink", link.display()));
        }
        Ok(())
    }

    /// Run a mutating tool invocation.
    ///
    /// Record mode returns a successful, empty [`RunOutput`] without spawning
    /// anything.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the tool cannot be spawned (apply mode only).
    pub fn run(&self, tool: &Tool) -> Result<RunOutput> {
        self.run_as(None, tool)
    }

    /// [`Effects::run`], with a note on why the command would run, shown in the plan.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the tool cannot be spawned (apply mode only).
    pub fn run_as(&self, intent: Option<&str>, tool: &Tool) -> Result<RunOutput> {
        let Some(r) = &self.recorder else {
            return tool.run();
        };
        let (program, args) = tool.command_line();
        push(
            r,
            Planned::Run {
                program,
                args,
                cwd: None,
                intent: intent.map(str::to_string),
            },
        );
        Ok(RunOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    /// Run a mutating command in `cwd` (see [`run_command`]).
    ///
    /// Record mode returns empty stdout without spawning anything.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the command fails (apply mode only).
    pub fn run_command(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
        let Some(r) = &self.recorder else {
            return run_command(program, args, cwd);
        };
        push(
            r,
            Planned::Run {
                program: program.to_string(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
                cwd: cwd.map(Path::to_path_buf),
                intent: None,
            },
        );
        Ok(String::new())
    }

    /// Answer a read-only question whose answer depends on earlier effects.
    ///
    /// Apply mode runs `probe`. Record mode cannot, since the effects it would
    /// observe were never performed, so it returns `assumed` — the answer the
    /// probe would give had they been.
    pub fn probe<T>(&self, assumed: T, probe: impl FnOnce() -> T) -> T {
        if self.is_recording() {
            assumed
        } else {
            probe()
        }
    }

    /// Render the recorded plan for people, with paths shown relative to `base`.
    #[must_use]
    pub fn render_plan(&self, base: &Path) -> String {
        let planned = self.planned();
        let mut out = String::new();
        if planned.is_empty() {
            out.push_str("Nothing to change.\n");
            return out;
        }
        out.push_str("Would change:\n");
        let rel = |p: &Path| p.strip_prefix(base).unwrap_or(p).display().to_string();
        for item in &planned {
            let _ = match item {
                Planned::CreateFile { path, lines } => {
                    writeln!(out, "  create   {} ({lines} lines)", rel(path))
                }
                Planned::UpdateFile {
                    path,
                    added,
                    removed,
                } => writeln!(out, "  update   {} (+{added} -{removed} lines)", rel(path)),
                Planned::GenerateFile { path, source } => {
                    writeln!(out, "  create   {} (from {source})", rel(path))
                }
                Planned::RemoveFile { path } => writeln!(out, "  remove   {}", rel(path)),
                Planned::RemoveDir { path } => writeln!(out, "  remove   {}/", rel(path)),
                Planned::CreateDir { path } => writeln!(out, "  mkdir    {}/", rel(path)),
                Planned::Rename { from, to } => {
                    writeln!(out, "  rename   {} -> {}", rel(from), rel(to))
                }
                Planned::Symlink { link, target } => {
                    writeln!(out, "  symlink  {} -> {target}", rel(link))
                }
                Planned::Step(message) => writeln!(out, "  step     {}", message.trim()),
                Planned::Run {
                    program,
                    args,
                    cwd,
                    intent,
                } => {
                    render_run(&mut out, program, args, cwd.as_deref(), intent.as_deref());
                    Ok(())
                }
            };
        }
        out.push_str("\nRun without --dry-run to execute.\n");
        out
    }
}

/// Render one planned subprocess. `rite hooks add` gets a structured view,
/// since the hook it registers is what a reader most needs to check.
fn render_run(
    out: &mut String,
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    intent: Option<&str>,
) {
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    if program == "rite"
        && let ["hooks", "add", rest @ ..] = args_ref.as_slice()
    {
        let flag = |name: &str| {
            rest.iter()
                .take_while(|a| **a != "--")
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| *w[0] == name)
                .map(|w| (*w[1]).to_string())
        };
        let hook_name = flag("--name")
            .or_else(|| flag("--description"))
            .unwrap_or_else(|| "(unnamed)".to_string());
        let command = rest
            .iter()
            .position(|a| *a == "--")
            .map(|i| rest[i + 1..].join(" "))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  hook     {hook_name} ({})",
            intent.unwrap_or("register")
        );
        let _ = writeln!(
            out,
            "             channel: {}",
            flag("--channel").unwrap_or_default()
        );
        let _ = writeln!(
            out,
            "             cwd:     {}",
            flag("--cwd").unwrap_or_default()
        );
        let _ = writeln!(out, "             command: {command}");
        return;
    }
    let _ = write!(out, "  run      {program} {}", args.join(" "));
    if let Some(dir) = cwd {
        let _ = write!(out, "  (in {})", dir.display());
    }
    if let Some(why) = intent {
        let _ = write!(out, "  # {why}");
    }
    out.push('\n');
}

/// Count lines added and removed going from `old` to `new` (as multisets —
/// a short summary, not a minimal diff).
fn line_delta(old: &str, new: &str) -> (usize, usize) {
    let mut counts: HashMap<&str, isize> = HashMap::new();
    for line in new.lines() {
        *counts.entry(line).or_default() += 1;
    }
    for line in old.lines() {
        *counts.entry(line).or_default() -= 1;
    }
    let added = counts
        .values()
        .filter(|c| **c > 0)
        .map(|c| c.unsigned_abs())
        .sum();
    let removed = counts
        .values()
        .filter(|c| **c < 0)
        .map(|c| c.unsigned_abs())
        .sum();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_performs_writes_and_commands() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        let marker = dir.path().join("marker");
        let fx = Effects::apply();

        fx.write(&file, "hello\n").expect("write");
        fx.run(&Tool::new("touch").arg(marker.to_str().expect("utf8")))
            .expect("run");

        assert_eq!(fs::read_to_string(&file).expect("read"), "hello\n");
        assert!(marker.exists(), "apply mode must run the command");
        assert!(fx.planned().is_empty(), "apply mode records nothing");
    }

    #[test]
    fn record_captures_writes_and_commands_without_performing_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existing = dir.path().join("existing.txt");
        let unchanged = dir.path().join("unchanged.txt");
        let new = dir.path().join("new.txt");
        let marker = dir.path().join("marker");
        fs::write(&existing, "one\ntwo\n").expect("seed");
        fs::write(&unchanged, "same\n").expect("seed");
        let fx = Effects::record();

        fx.write(&existing, "one\nthree\nfour\n").expect("write");
        fx.write(&unchanged, "same\n").expect("write");
        fx.write(&new, "x\n").expect("write");
        fx.remove_file(&unchanged).expect("remove");
        fx.run_as(
            Some("create"),
            &Tool::new("touch").arg(marker.to_str().expect("utf8")),
        )
        .expect("run");
        fx.run_command("touch", &[marker.to_str().expect("utf8")], None)
            .expect("run_command");

        // Nothing happened on disk.
        assert_eq!(fs::read_to_string(&existing).expect("read"), "one\ntwo\n");
        assert!(unchanged.exists());
        assert!(!new.exists());
        assert!(!marker.exists(), "record mode must not run the command");

        // Later reads see the planned state.
        assert_eq!(
            fx.read_to_string(&existing).expect("overlay"),
            "one\nthree\nfour\n"
        );
        assert!(fx.exists(&new));
        assert!(!fx.exists(&unchanged));

        let planned = fx.planned();
        assert_eq!(
            planned[0],
            Planned::UpdateFile {
                path: existing,
                added: 2,
                removed: 1
            }
        );
        // An identical write is not a change.
        assert_eq!(
            planned[1],
            Planned::CreateFile {
                path: new,
                lines: 1
            }
        );
        assert_eq!(planned[2], Planned::RemoveFile { path: unchanged });
        assert!(matches!(&planned[3], Planned::Run { program, intent, .. }
            if program == "touch" && intent.as_deref() == Some("create")));
        assert!(matches!(&planned[4], Planned::Run { program, .. } if program == "touch"));
        assert_eq!(planned.len(), 5);
    }

    #[test]
    fn write_generated_defers_the_producer_to_apply_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join(".gitignore");

        let fx = Effects::record();
        fx.write_generated(&file, "https://example.invalid", || {
            panic!("record mode must not produce content")
        })
        .expect("record");
        assert!(!file.exists());
        fx.report("not printed in record mode");
        assert_eq!(
            fx.planned(),
            vec![Planned::GenerateFile {
                path: file.clone(),
                source: "https://example.invalid".to_string()
            }]
        );
        assert!(
            fx.render_plan(dir.path())
                .contains("create   .gitignore (from https://example.invalid)")
        );

        Effects::apply()
            .write_generated(&file, "x", || Ok("target/\n".to_string()))
            .expect("apply");
        assert_eq!(fs::read_to_string(&file).expect("read"), "target/\n");
    }

    #[test]
    fn probe_assumes_in_record_mode() {
        assert!(Effects::record().probe(true, || false));
        assert!(!Effects::apply().probe(true, || false));
    }

    #[test]
    fn plan_renders_hook_registration_fields() {
        let fx = Effects::record();
        fx.run_as(
            Some("create"),
            &Tool::new("rite").args(&[
                "hooks",
                "add",
                "--description",
                "edict:demo:responder",
                "--name",
                "edict:demo:responder",
                "--channel",
                "demo",
                "--cwd",
                "/p",
                "--",
                "vessel",
                "spawn",
                "--cwd",
                "/p",
                "--",
                "edict",
                "run",
                "responder",
            ]),
        )
        .expect("run");
        let plan = fx.render_plan(Path::new("/p"));
        assert!(
            plan.contains("hook     edict:demo:responder (create)"),
            "{plan}"
        );
        assert!(plan.contains("channel: demo"), "{plan}");
        assert!(plan.contains("cwd:     /p"), "{plan}");
        assert!(
            plan.contains("command: vessel spawn --cwd /p -- edict run responder"),
            "{plan}"
        );
        assert!(plan.contains("Run without --dry-run to execute."));
    }
}
