use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::effects::Effects;
use crate::error::ExitError;
use crate::layout::Layout;
use crate::subprocess::{Tool, run_command};
use crate::template::{TemplateContext, render_workflow_doc, update_managed_section};

#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools, reason = "CLI argument flag struct")]
pub struct SyncArgs {
    /// Project root directory
    #[arg(long)]
    pub project_root: Option<PathBuf>,
    /// Check mode: exit non-zero if anything is stale, without making changes
    #[arg(long)]
    pub check: bool,
    /// Preview: list the files, migrations, rite hooks and commit sync would
    /// change, then exit 0 without changing anything
    #[arg(long, conflicts_with = "check")]
    pub dry_run: bool,
    /// Disable auto-commit (default: enabled)
    #[arg(long)]
    pub no_commit: bool,
    /// Allow registering live rite hooks for a project rooted under the
    /// system temp directory (refused by default). Has no effect when
    /// `RITE_DATA_DIR` is set.
    #[arg(long)]
    pub allow_live_hooks: bool,
}

/// Embedded workflow docs
pub const WORKFLOW_DOCS: &[(&str, &str)] = &[
    ("triage.md", include_str!("../templates/docs/triage.md")),
    ("start.md", include_str!("../templates/docs/start.md")),
    ("update.md", include_str!("../templates/docs/update.md")),
    ("finish.md", include_str!("../templates/docs/finish.md")),
    (
        "worker-loop.md",
        include_str!("../templates/docs/worker-loop.md"),
    ),
    ("planning.md", include_str!("../templates/docs/planning.md")),
    ("scout.md", include_str!("../templates/docs/scout.md")),
    ("proposal.md", include_str!("../templates/docs/proposal.md")),
    (
        "review-request.md",
        include_str!("../templates/docs/review-request.md"),
    ),
    (
        "review-response.md",
        include_str!("../templates/docs/review-response.md"),
    ),
    (
        "security-review.md",
        include_str!("../templates/docs/security-review.md"),
    ),
    (
        "merge-check.md",
        include_str!("../templates/docs/merge-check.md"),
    ),
    (
        "preflight.md",
        include_str!("../templates/docs/preflight.md"),
    ),
    (
        "cross-channel.md",
        include_str!("../templates/docs/cross-channel.md"),
    ),
    (
        "report-issue.md",
        include_str!("../templates/docs/report-issue.md"),
    ),
    ("groom.md", include_str!("../templates/docs/groom.md")),
    ("mission.md", include_str!("../templates/docs/mission.md")),
    (
        "coordination.md",
        include_str!("../templates/docs/coordination.md"),
    ),
];

/// Embedded design docs
pub const DESIGN_DOCS: &[(&str, &str)] = &[(
    "cli-conventions.md",
    include_str!("../templates/design/cli-conventions.md"),
)];

impl SyncArgs {
    /// Run the sync command: detect staleness and update managed files, hooks, and docs.
    ///
    /// # Errors
    ///
    /// Returns `Err` if config loading, file I/O, or a subprocess invocation fails,
    /// or if `--check` is set and components are out of sync.
    ///
    /// With `--dry-run`, every change is recorded by [`Effects`] instead of
    /// performed, and the plan is printed.
    ///
    /// # Panics
    ///
    /// Panics if the current working directory cannot be determined.
    pub fn execute(&self) -> Result<()> {
        let project_root = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current dir"));

        let effects = if self.dry_run {
            Effects::record()
        } else {
            Effects::apply()
        };

        // Detect maw v2 bare repo
        if crate::config::find_config(&project_root.join("ws/default")).is_some() {
            return self.handle_bare_repo(&effects, &project_root);
        }

        self.sync_project(&effects, &project_root)?;
        if effects.is_recording() {
            print!("{}", effects.render_plan(&project_root));
        } else if !self.check {
            println!("Sync complete");
        }
        Ok(())
    }

    /// Sync one (non-bare) project, performing or recording each change via `fx`.
    fn sync_project(&self, fx: &Effects, project_root: &Path) -> Result<()> {
        let project_root = project_root.to_path_buf();

        // Detect the on-disk workspace layout (bare ws/ vs root .maw/workspaces).
        // When reached via handle_bare_repo's `maw exec default -- edict sync`,
        // project_root is the bare trunk (…/ws/default), which detect() recognizes.
        let layout = Layout::detect(&project_root);

        // Check for agents dir — accept new (.agents/edict/) or legacy (.agents/botbox/)
        let agents_dir_edict = project_root.join(".agents/edict");
        let agents_dir_legacy = project_root.join(".agents/botbox");
        if !agents_dir_edict.exists() && !agents_dir_legacy.exists() {
            return Err(ExitError::Other(
                "No .agents/edict/ found. Run `edict init` first.".to_string(),
            )
            .into());
        }

        // Load config (.edict.toml preferred, legacy names as fallback)
        let config_path = crate::config::find_config(&project_root)
            .ok_or_else(|| ExitError::Config("No .edict.toml or .botbox.toml found".to_string()))?;
        let config = Config::load(&config_path)
            .with_context(|| format!("Failed to parse {}", config_path.display()))?;

        // Migrate .botbox.json -> .edict.toml if needed (JSON is oldest legacy)
        let json_path = project_root.join(crate::config::CONFIG_JSON);
        let toml_path = project_root.join(crate::config::CONFIG_TOML);
        if json_path.exists() && !toml_path.exists() {
            let json_content = fs::read_to_string(&json_path)?;
            match crate::config::json_to_toml(&json_content) {
                Ok(toml_content) => {
                    fx.write(&toml_path, &toml_content)?;
                    fx.remove_file(&json_path)?;
                    fx.announce("Migrated .botbox.json -> .edict.toml");
                }
                Err(e) => {
                    tracing::warn!("failed to migrate .botbox.json to .edict.toml: {e}");
                }
            }
        }

        // Migrate .botbox.toml -> .edict.toml (botbox era → edict era)
        let legacy_toml_path = project_root.join(crate::config::CONFIG_TOML_LEGACY);
        if legacy_toml_path.exists() && !toml_path.exists() {
            match fx.rename(&legacy_toml_path, &toml_path) {
                Ok(()) => fx.announce("Migrated .botbox.toml -> .edict.toml"),
                Err(e) => tracing::warn!("failed to rename .botbox.toml to .edict.toml: {e}"),
            }
        }

        // Migrate .agents/botbox/ -> .agents/edict/ (botbox era → edict era)
        if agents_dir_legacy.exists() && !agents_dir_edict.exists() {
            match fx.rename(&agents_dir_legacy, &agents_dir_edict) {
                Ok(()) => fx.announce("Migrated .agents/botbox/ -> .agents/edict/"),
                Err(e) => tracing::warn!("failed to rename .agents/botbox/ to .agents/edict/: {e}"),
            }
        }

        // Resolved agents dir (after any migration above)
        let agents_dir = if agents_dir_edict.exists() {
            agents_dir_edict
        } else {
            agents_dir_legacy
        };

        // Check staleness for each component
        let docs_stale = Self::check_docs_staleness(&agents_dir, layout)?;
        let managed_stale = Self::check_managed_section_staleness(&project_root, &config, layout)?;
        let design_docs_stale = Self::check_design_docs_staleness(&agents_dir)?;

        let any_stale = docs_stale || managed_stale || design_docs_stale;

        if self.check {
            if any_stale {
                let mut parts = Vec::new();
                if docs_stale {
                    parts.push("workflow docs");
                }
                if managed_stale {
                    parts.push("AGENTS.md managed section");
                }
                if design_docs_stale {
                    parts.push("design docs");
                }
                tracing::warn!(components = %parts.join(", "), "stale components detected");
                return Err(ExitError::new(1, "Project is out of sync".to_string()).into());
            }
            println!("All components up to date");
            return Ok(());
        }

        // Clean up per-repo hooks (now managed globally)
        self.cleanup_per_repo_hooks(fx, &project_root)?;

        // Perform updates
        let mut changed_files = Vec::new();

        // Seen through fx, so a dry-run plans against the config a migration
        // above would have renamed into place.
        let active_config_path = if fx.exists(&toml_path) {
            toml_path
        } else {
            crate::config::find_config(&project_root).unwrap_or_else(|| config_path.clone())
        };
        if migrate_retired_model_defaults(fx, &active_config_path)? {
            changed_files.push(".edict.toml");
            fx.announce("Updated retired model defaults");
        }
        if migrate_retired_reviewer_config(fx, &active_config_path)? {
            changed_files.push(".edict.toml");
            fx.announce("Removed retired reviewer-loop configuration");
        }

        if docs_stale {
            Self::sync_workflow_docs(fx, &agents_dir, layout)?;
            changed_files.push(".agents/edict/*.md");
            fx.announce("Updated workflow docs");
        }

        if managed_stale {
            Self::sync_managed_section(fx, &project_root, &config, layout)?;
            changed_files.push("AGENTS.md");
            fx.announce("Updated AGENTS.md managed section");
        }

        if design_docs_stale {
            Self::sync_design_docs(fx, &agents_dir)?;
            changed_files.push(".agents/edict/design/*.md");
            fx.announce("Updated design docs");
        }

        // Clean up legacy JS artifacts (scripts, shell hooks)
        self.cleanup_legacy_artifacts(fx, &agents_dir, &mut changed_files);

        // Migrate rite hooks from bun .mjs to edict run
        migrate_rite_hooks(fx, &config, self.allow_live_hooks);

        // The ambient reviewer loop is retired. Only remove hooks that Edict
        // can prove it owns; a project's independently managed mention hooks
        // are outside this migration's authority.
        retire_owned_reviewer_hooks(fx, &config);

        // Migrate rite hooks from botbox: descriptions to edict: descriptions
        migrate_botbox_rite_hooks_to_edict(fx, &config, &project_root, self.allow_live_hooks);

        // Fix hook --cwd for maw v2 (ws/default → repo root)
        migrate_hook_cwd(fx, &config, &project_root, self.allow_live_hooks);

        // Migrate router hook claim from agent://{name}-router → agent://{name}-dev
        migrate_router_hook_claim(fx, &config, &project_root, self.allow_live_hooks);

        // Migrate botty → vessel (config key + rite hooks)
        if !self.check {
            migrate_vessel_hooks(
                fx,
                &config,
                &project_root,
                &active_config_path,
                self.allow_live_hooks,
            );
        }

        // Migrate BOTBUS_* → RITE_* env vars in hook commands
        if !self.check {
            migrate_botbus_env_hooks(fx, &config, &project_root, self.allow_live_hooks);
        }

        // Forward RITE_BATCH_* so spawned agents can resolve their reply anchor
        if !self.check {
            migrate_hook_reply_env(fx, &config, &project_root, self.allow_live_hooks);
        }

        // Ensure the router hook exists — a project channel with no responder
        // silently answers nobody (see ensure_router_hook). This one propagates:
        // if the live-hook guard refuses, sync should fail loudly rather than
        // report success while the channel answers nobody.
        if !self.check {
            ensure_router_hook(fx, &config, &project_root, self.allow_live_hooks)?;
        }

        // Migrate beads → bones (config, data, tooling files)
        if !self.check {
            migrate_beads_to_bones(fx, &project_root, &active_config_path)?;
        }

        // Auto-commit if changes were made
        if !changed_files.is_empty() && !self.no_commit {
            Self::auto_commit(fx, &project_root, &changed_files)?;
        }

        Ok(())
    }

    fn handle_bare_repo(&self, fx: &Effects, project_root: &Path) -> Result<()> {
        // Canonicalize project_root to prevent path traversal
        let project_root = project_root
            .canonicalize()
            .context("canonicalizing project root")?;

        // Validate this is actually an edict project
        if crate::config::find_config(&project_root).is_none()
            && crate::config::find_config(&project_root.join("ws/default")).is_none()
        {
            anyhow::bail!(
                "not an edict project: no .edict.toml or .botbox.toml found in {}",
                project_root.display()
            );
        }

        let mut args = vec!["exec", "default", "--", "edict", "sync"];
        if self.check {
            args.push("--check");
        }
        if self.no_commit {
            args.push("--no-commit");
        }
        if self.dry_run {
            args.push("--dry-run");
        }
        if self.allow_live_hooks {
            args.push("--allow-live-hooks");
        }

        // Not an effect: the inner sync applies or records by the same flags.
        let inner = run_command("maw", &args, Some(&project_root))?;
        if fx.is_recording() {
            println!("ws/default:");
            print!("{inner}");
            println!("\nBare repo root:");
        }

        // Clean up stale legacy config files at bare repo root.
        //
        // After migration runs inside ws/default/, the bare root may still have stale
        // .botbox.json or .botbox.toml files. Agents resolving config from the project root
        // would find these before the authoritative ws/default/.edict.toml.
        //
        // Only remove when ws/default has a config, ensuring the authoritative config is in place.
        let ws_has_config = crate::config::find_config(&project_root.join("ws/default")).is_some();
        for stale_name in &[
            crate::config::CONFIG_JSON,
            crate::config::CONFIG_TOML_LEGACY,
        ] {
            let stale_path = project_root.join(stale_name);
            if stale_path.exists() && ws_has_config {
                if self.check {
                    tracing::warn!(
                        "stale {stale_name} at bare repo root (will be removed on sync)"
                    );
                    return Err(
                        ExitError::new(1, format!("Stale {stale_name} at bare repo root")).into(),
                    );
                }
                match fx.remove_file(&stale_path) {
                    Ok(()) => fx.announce(format!(
                        "Removed stale {stale_name} from bare repo root \
                         (authoritative config lives in ws/default/)"
                    )),
                    Err(e) => {
                        tracing::warn!("failed to remove stale {stale_name} at bare root: {e}");
                    }
                }
            }
        }

        // Create stubs at bare root
        let stub_agents = project_root.join("AGENTS.md");
        let stub_content = "**Do not edit the root AGENTS.md for memories or instructions. Use the AGENTS.md in ws/default/.**\n@ws/default/AGENTS.md\n";

        if !stub_agents.exists() {
            fx.write(&stub_agents, stub_content)?;
            fx.announce("Created bare-root AGENTS.md stub");
        }

        // Symlink .claude directory — use atomic approach to avoid TOCTOU
        let root_claude_dir = project_root.join(".claude");
        let ws_claude_dir = project_root.join("ws/default/.claude");

        if ws_claude_dir.exists() {
            // Check if already a correct symlink
            let needs_symlink = fs::read_link(&root_claude_dir)
                .map_or(true, |target| target != Path::new("ws/default/.claude"));

            if needs_symlink {
                // Atomic: temp symlink, then rename over the target
                fx.symlink_replace("ws/default/.claude", &root_claude_dir)?;
                fx.announce("Symlinked .claude → ws/default/.claude");
            }
        }

        // Symlink .pi directory
        let root_pi_dir = project_root.join(".pi");
        let ws_pi_dir = project_root.join("ws/default/.pi");

        if ws_pi_dir.exists() {
            let needs_symlink = fs::read_link(&root_pi_dir)
                .map_or(true, |target| target != Path::new("ws/default/.pi"));

            if needs_symlink {
                fx.symlink_replace("ws/default/.pi", &root_pi_dir)?;
                fx.announce("Symlinked .pi → ws/default/.pi");
            }
        }

        if fx.is_recording() {
            print!("{}", fx.render_plan(&project_root));
        }
        Ok(())
    }

    /// Remove legacy JS-era artifacts that are no longer needed.
    /// The Rust rewrite builds loops into the binary, so .mjs scripts and
    /// shell hook wrappers are dead weight.
    fn cleanup_legacy_artifacts(
        &self,
        fx: &Effects,
        agents_dir: &Path,
        changed_files: &mut Vec<&str>,
    ) {
        // Remove .agents/botbox/scripts/ (JS loop scripts)
        let scripts_dir = agents_dir.join("scripts");
        if scripts_dir.is_dir() {
            if self.check {
                tracing::warn!("legacy scripts/ directory exists (will be removed on sync)");
            } else {
                match fx.remove_dir_all(&scripts_dir) {
                    Ok(()) => {
                        fx.announce("Removed legacy scripts/ directory");
                        changed_files.push(".agents/botbox/scripts/");
                    }
                    Err(e) => tracing::warn!("failed to remove legacy scripts/: {e}"),
                }
            }
        }

        // Remove .agents/botbox/hooks/ (shell hook scripts — now built into botbox binary)
        let hooks_dir = agents_dir.join("hooks");
        if hooks_dir.is_dir() {
            if self.check {
                tracing::warn!("legacy hooks/ directory exists (will be removed on sync)");
            } else {
                match fx.remove_dir_all(&hooks_dir) {
                    Ok(()) => {
                        fx.announce("Removed legacy hooks/ directory");
                        changed_files.push(".agents/botbox/hooks/");
                    }
                    Err(e) => tracing::warn!("failed to remove legacy hooks/: {e}"),
                }
            }
        }

        // Remove stale version markers from JS era
        for marker in &[".scripts-version", ".hooks-version"] {
            let path = agents_dir.join(marker);
            if path.exists() && !self.check {
                let _ = fx.remove_file(&path);
            }
        }
    }

    fn check_docs_staleness(agents_dir: &Path, layout: Layout) -> Result<bool> {
        let version_file = agents_dir.join(".version");
        let current = compute_docs_version(layout);

        if !version_file.exists() {
            return Ok(true);
        }

        let installed = fs::read_to_string(&version_file)?.trim().to_string();
        Ok(installed != current)
    }

    fn check_managed_section_staleness(
        project_root: &Path,
        config: &Config,
        layout: Layout,
    ) -> Result<bool> {
        let agents_md = project_root.join("AGENTS.md");
        if !agents_md.exists() {
            return Ok(false); // No AGENTS.md to update
        }

        let content = fs::read_to_string(&agents_md)?;
        let ctx = TemplateContext::from_config(config, layout);
        let updated = update_managed_section(&content, &ctx)?;

        Ok(content != updated)
    }

    /// Clean up per-repo hooks that are now managed globally.
    /// Removes botbox hooks from per-repo .claude/settings.json and .pi/extensions/.
    fn cleanup_per_repo_hooks(&self, fx: &Effects, project_root: &Path) -> Result<()> {
        if self.check {
            return Ok(());
        }

        // Clean up per-repo .claude/settings.json botbox hooks
        let settings_path = project_root.join(".claude/settings.json");
        if settings_path.exists() {
            let content = fs::read_to_string(&settings_path)?;
            if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut changed = false;
                if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                    for (_event, entries) in hooks.iter_mut() {
                        if let Some(arr) = entries.as_array_mut() {
                            let before = arr.len();
                            arr.retain(|entry| {
                                !entry["hooks"].as_array().is_some_and(|hooks| {
                                    hooks.iter().any(|h| {
                                        let cmd = &h["command"];
                                        cmd.as_str().map_or_else(
                                            || {
                                                cmd.as_array().is_some_and(|a| {
                                                    a.len() >= 3
                                                        && a[0].as_str() == Some("botbox")
                                                        && a[1].as_str() == Some("hooks")
                                                        && a[2].as_str() == Some("run")
                                                })
                                            },
                                            |s| s.contains("botbox hooks run"),
                                        )
                                    })
                                })
                            });
                            if arr.len() != before {
                                changed = true;
                            }
                        }
                    }
                    // Remove empty event arrays
                    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
                }

                if changed {
                    // Remove hooks key entirely if empty
                    if settings
                        .get("hooks")
                        .and_then(|h| h.as_object())
                        .is_some_and(serde_json::Map::is_empty)
                    {
                        settings
                            .as_object_mut()
                            .expect("settings is a JSON object")
                            .remove("hooks");
                    }

                    // Only write back if there's other content; delete if empty
                    if settings.as_object().is_some_and(serde_json::Map::is_empty) {
                        fx.remove_file(&settings_path)?;
                        // Also remove .claude dir if empty
                        let claude_dir = project_root.join(".claude");
                        if claude_dir.exists() && fs::read_dir(&claude_dir)?.next().is_none() {
                            fx.remove_dir(&claude_dir)?;
                        }
                    } else {
                        fx.write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
                    }
                    fx.announce(
                        "Cleaned up per-repo botbox hooks from .claude/settings.json (now managed globally via `botbox hooks install`)",
                    );
                }
            }
        }

        // Clean up per-repo Pi extension
        let pi_ext = project_root.join(".pi/extensions/botbox-hooks.ts");
        if pi_ext.exists() {
            fx.remove_file(&pi_ext)?;
            // Clean up empty dirs
            let pi_ext_dir = project_root.join(".pi/extensions");
            if pi_ext_dir.exists() && fs::read_dir(&pi_ext_dir)?.next().is_none() {
                fx.remove_dir(&pi_ext_dir)?;
            }
            let pi_dir = project_root.join(".pi");
            if pi_dir.exists() && fs::read_dir(&pi_dir)?.next().is_none() {
                fx.remove_dir(&pi_dir)?;
            }
            fx.announce(
                "Cleaned up per-repo Pi extension (now managed globally via `botbox hooks install`)",
            );
        }

        Ok(())
    }

    fn check_design_docs_staleness(agents_dir: &Path) -> Result<bool> {
        let version_file = agents_dir.join("design/.design-docs-version");
        let current = compute_design_docs_version();

        if !version_file.exists() {
            return Ok(true);
        }

        let installed = fs::read_to_string(&version_file)?.trim().to_string();
        Ok(installed != current)
    }

    fn sync_workflow_docs(fx: &Effects, agents_dir: &Path, layout: Layout) -> Result<()> {
        for (name, content) in WORKFLOW_DOCS {
            let path = agents_dir.join(name);
            let rendered = render_workflow_doc(content, layout)
                .with_context(|| format!("Failed to render {name}"))?;
            fx.write(&path, rendered)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }

        let version = compute_docs_version(layout);
        fx.write(&agents_dir.join(".version"), version)?;

        // These files belonged to the retired ambient reviewer loop. They are
        // generated artifacts, so sync may remove them without touching
        // project-authored workflow guidance.
        for legacy in [
            "review-loop.md",
            "prompts/reviewer.md",
            "prompts/reviewer-security.md",
            "prompts/.prompts-version",
        ] {
            let path = agents_dir.join(legacy);
            if path.exists() {
                fx.remove_file(&path)?;
            }
        }
        let prompts_dir = agents_dir.join("prompts");
        if prompts_dir.exists() && fs::read_dir(&prompts_dir)?.next().is_none() {
            fx.remove_dir(&prompts_dir)?;
        }

        Ok(())
    }

    fn sync_managed_section(
        fx: &Effects,
        project_root: &Path,
        config: &Config,
        layout: Layout,
    ) -> Result<()> {
        let agents_md = project_root.join("AGENTS.md");
        if !agents_md.exists() {
            return Ok(()); // Skip if no AGENTS.md
        }

        let content = fx.read_to_string(&agents_md)?;
        let ctx = TemplateContext::from_config(config, layout);
        let updated = update_managed_section(&content, &ctx)?;

        fx.write(&agents_md, updated)?;
        Ok(())
    }

    // sync_hooks removed — hooks are now installed globally via `botbox hooks install`

    fn sync_design_docs(fx: &Effects, agents_dir: &Path) -> Result<()> {
        let design_dir = agents_dir.join("design");
        fx.create_dir_all(&design_dir)?;

        for (name, content) in DESIGN_DOCS {
            let path = design_dir.join(name);
            fx.write(&path, content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }

        let version = compute_design_docs_version();
        fx.write(&design_dir.join(".design-docs-version"), version)?;

        Ok(())
    }

    fn auto_commit(fx: &Effects, project_root: &Path, changed_files: &[&str]) -> Result<()> {
        let vcs = detect_vcs(project_root);
        if vcs == Vcs::None {
            return Ok(()); // No VCS found, skip commit
        }

        // All paths that edict sync may touch — git add is a no-op for unchanged files
        let managed_paths: &[&str] = &[
            ".agents/edict/",
            "AGENTS.md",
            ".sealignore",
            ".edict.toml",
            ".edict.json",
            ".gitignore",
        ];

        // Build a human-readable summary from the caller's changed_files list
        let files_str: String = changed_files
            .join(", ")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let message = format!("chore: edict sync (updated {files_str})");

        match vcs {
            Vcs::Git => {
                // Stage managed paths that exist — git add errors on missing pathspecs
                let existing: Vec<&str> = managed_paths
                    .iter()
                    .copied()
                    .filter(|p| project_root.join(p).exists())
                    .collect();
                if existing.is_empty() {
                    return Ok(());
                }
                let mut args = vec!["add", "--"];
                args.extend_from_slice(&existing);
                fx.run_command("git", &args, Some(project_root))?;

                // Only commit if there are staged changes
                // A dry-run staged nothing, so assume the add above would have.
                let staged = fx.probe(true, || {
                    run_command("git", &["diff", "--cached", "--quiet"], Some(project_root))
                        .is_err()
                });
                if staged {
                    // diff --cached --quiet exits 1 when there are staged changes
                    fx.run_command("git", &["commit", "-m", &message], Some(project_root))?;
                }
            }
            Vcs::None => unreachable!(),
        }

        Ok(())
    }
}

/// Replace known retired model defaults in their expected tier. Custom model
/// entries and retired-looking models in other tiers are left untouched.
fn migrate_retired_model_defaults(fx: &Effects, config_path: &Path) -> Result<bool> {
    let source = fx
        .read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parsing {}", config_path.display()))?;

    let Some(models) = document
        .get_mut("models")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok(false);
    };

    let mut changed = false;
    let migrations = [
        (
            "fast",
            "openai-codex/gpt-5.3-codex-spark",
            "openai-codex/gpt-5.6-luna",
        ),
        (
            "balanced",
            "anthropic/claude-sonnet-4-6",
            "anthropic/claude-sonnet-5:medium",
        ),
        (
            "balanced",
            "openai-codex/gpt-5.3-codex",
            "openai-codex/gpt-5.6-terra",
        ),
        (
            "balanced",
            "openai-codex/gpt-5.4",
            "openai-codex/gpt-5.6-terra",
        ),
        (
            "strong",
            "anthropic/claude-opus-4-6",
            "anthropic/claude-opus-4-8:high",
        ),
        (
            "strong",
            "openai-codex/gpt-5.3-codex",
            "openai-codex/gpt-5.6-sol",
        ),
    ];

    for (tier, retired_slug, replacement) in migrations {
        let Some(tier_models) = models.get_mut(tier).and_then(toml_edit::Item::as_array_mut) else {
            continue;
        };

        for model in tier_models.iter_mut() {
            let is_retired = model
                .as_str()
                .is_some_and(|value| value.split(':').next() == Some(retired_slug));
            if is_retired {
                *model = toml_edit::Value::from(replacement);
                changed = true;
            }
        }
    }

    if changed {
        fx.write(config_path, document.to_string())
            .with_context(|| format!("writing {}", config_path.display()))?;
    }

    Ok(changed)
}

/// Remove the configuration block that only configured the retired ambient
/// reviewer loop. `review.reviewers` remains: it names the reviewers Seal must
/// collect votes from, including the dedicated Daybreak security reviewer.
fn migrate_retired_reviewer_config(fx: &Effects, config_path: &Path) -> Result<bool> {
    let source = fx
        .read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parsing {}", config_path.display()))?;

    let Some(agents) = document
        .get_mut("agents")
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return Ok(false);
    };

    if agents.remove("reviewer").is_none() {
        return Ok(false);
    }

    fx.write(config_path, document.to_string())
        .with_context(|| format!("writing {}", config_path.display()))?;
    Ok(true)
}

/// Remove a named reviewer-loop hook only when it is demonstrably owned by
/// Edict. A project may have its own `@project-security` automation, so a
/// matching mention alone is deliberately insufficient authority to remove it.
fn retire_owned_reviewer_hooks(fx: &Effects, config: &Config) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(output) if output.success() => output,
        _ => return,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(value) => value,
        Err(_) => return,
    };
    let Some(hooks) = parsed.get("hooks").and_then(|hooks| hooks.as_array()) else {
        return;
    };

    for hook in hooks {
        if !is_owned_reviewer_hook(hook, &config.project.name) {
            continue;
        }
        let Some(id) = hook.get("id").and_then(|id| id.as_str()) else {
            continue;
        };
        match fx.run(&Tool::new("rite").args(&["hooks", "remove", id])) {
            Ok(output) if output.success() => {
                fx.announce(format!("Retired Edict reviewer hook {id}"));
            }
            Ok(output) => {
                tracing::warn!(hook_id = %id, stderr = %output.stderr, "failed to retire Edict reviewer hook");
            }
            Err(error) => {
                tracing::warn!(hook_id = %id, %error, "failed to retire Edict reviewer hook");
            }
        }
    }
}

fn is_owned_reviewer_hook(hook: &serde_json::Value, project_name: &str) -> bool {
    let expected_prefix = format!("edict:{project_name}:reviewer-");
    hook.get("owner").and_then(|owner| owner.as_str()) == Some("edict")
        && hook
            .get("name")
            .and_then(|name| name.as_str())
            .is_some_and(|name| name.starts_with(&expected_prefix))
}

/// Migrate rite hooks from `botbox:` descriptions to `edict:` descriptions.
///
/// Finds hooks with `botbox:{name}:responder` or `botbox:{name}:reviewer-*` descriptions,
/// removes them, and re-registers with `edict:` prefix and `edict run` commands.
/// Called during `edict sync` on projects that were previously set up with `botbox`.
fn migrate_botbox_rite_hooks_to_edict(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    allow_live_hooks: bool,
) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return;
    };

    let name = &config.project.name;

    // Resolve the correct cwd (bare root or project root)
    let bare_root = if project_root.ends_with("ws/default") {
        project_root
            .parent()
            .and_then(Path::parent)
            .filter(|r| r.join(".manifold").exists())
    } else if project_root.join(".manifold").exists() {
        Some(project_root)
    } else {
        None
    };
    let root_str = bare_root.map_or_else(
        || project_root.display().to_string(),
        |r| r.display().to_string(),
    );

    for hook in hooks {
        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");

        // Only process botbox-era hooks for this project
        if !desc.starts_with(&format!("botbox:{name}:")) {
            continue;
        }

        let Some(id) = hook.get("id").and_then(|i| i.as_str()) else {
            continue;
        };

        // Remove old botbox hook
        if fx
            .run(&Tool::new("rite").args(&["hooks", "remove", id]))
            .is_err()
        {
            tracing::warn!(hook_id = %id, "failed to remove botbox-era hook during edict migration");
            continue;
        }

        let agent = config.default_agent();
        if desc.ends_with(":responder") {
            let responder_ml = config
                .agents
                .responder
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            let _ = super::init::register_router_hook(
                fx,
                &root_str,
                &root_str,
                name,
                &agent,
                responder_ml,
                allow_live_hooks,
            );
            fx.announce(format!("  Migrated hook {desc} → edict:{name}:responder"));
        } else if desc.starts_with(&format!("botbox:{name}:reviewer-")) {
            fx.announce(format!("  Retired legacy reviewer hook {desc}"));
        }
    }
}

/// Migrate rite hooks from legacy formats to current `edict run` commands with descriptions.
///
/// Lists all hooks for this project's channel, identifies legacy hooks
/// (bun-based, old naming, missing descriptions), removes them, and
/// re-registers via `ensure_rite_hook` with proper descriptions for
/// future idempotent management.
fn migrate_rite_hooks(fx: &Effects, config: &Config, allow_live_hooks: bool) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return, // rite not available, skip silently
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return;
    };

    let name = &config.project.name;
    let agent = config.default_agent();
    let env_inherit = "RITE_CHANNEL,RITE_MESSAGE_ID,RITE_HOOK_ID,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";

    for hook in hooks {
        let id = match hook.get("id").and_then(|i| i.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let channel = hook.get("channel").and_then(|c| c.as_str()).unwrap_or("");

        // Only migrate hooks for this project's channel
        if channel != name {
            continue;
        }

        // Skip hooks that already have an edict: or botbox: description (already migrated by
        // migrate_rite_hooks or migrate_botbox_rite_hooks_to_edict respectively)
        let existing_desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if existing_desc.starts_with("edict:") || existing_desc.starts_with("botbox:") {
            continue;
        }

        let Some(cmd) = hook.get("command").and_then(|c| c.as_array()) else {
            continue;
        };

        let cmd_strs: Vec<&str> = cmd.iter().filter_map(|v| v.as_str()).collect();

        // Determine what kind of hook this is
        let is_router = cmd_strs.iter().any(|s| {
            s.contains("responder") || s.contains("respond.mjs") || s.contains("router.mjs")
        });
        if !is_router {
            continue;
        }

        let spawn_cwd = cmd_strs
            .windows(2)
            .find(|w| w[0] == "--cwd")
            .map_or(".", |w| w[1]);

        // Remove old hook (ensure_rite_hook handles dedup by description,
        // but these legacy hooks have no description so we remove manually)
        let remove = fx.run(&Tool::new("rite").args(&["hooks", "remove", &id]));

        if remove.is_err() || !remove.as_ref().expect("remove is Ok here").success() {
            tracing::warn!(hook_id = %id, "failed to remove legacy hook");
            continue;
        }

        reregister_legacy_router_hook(
            fx,
            config,
            name,
            &agent,
            env_inherit,
            spawn_cwd,
            &id,
            allow_live_hooks,
        );
    }
}

/// Re-register a legacy router hook with the current `edict run responder` command.
#[allow(
    clippy::too_many_arguments,
    reason = "internal helper, plain param list is clearest"
)]
fn reregister_legacy_router_hook(
    fx: &Effects,
    config: &Config,
    name: &str,
    agent: &str,
    env_inherit: &str,
    spawn_cwd: &str,
    id: &str,
    allow_live_hooks: bool,
) {
    let claim_uri = format!("agent://{name}-dev");
    let spawn_name = format!("{name}-responder");
    let description = format!("edict:{name}:responder");
    let responder_ml = config
        .agents
        .responder
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    let mut router_args: Vec<&str> = vec![
        "--agent",
        agent,
        "--channel",
        name,
        "--claim",
        &claim_uri,
        "--claim-owner",
        agent,
        "--cwd",
        spawn_cwd,
        "--ttl",
        "600",
        "--",
        "vessel",
        "spawn",
        "--env-inherit",
        env_inherit,
    ];
    if let Some(limit) = responder_ml {
        router_args.push("--memory-limit");
        router_args.push(limit);
    }
    router_args.extend_from_slice(&[
        "--name",
        &spawn_name,
        "--cwd",
        spawn_cwd,
        "--",
        "edict",
        "run",
        "responder",
    ]);

    match crate::subprocess::ensure_rite_hook_with(fx, &description, &router_args, allow_live_hooks)
    {
        Ok(_) => fx.announce(format!("  Migrated router hook {id} → edict run responder")),
        Err(e) => tracing::warn!("failed to re-register router hook: {e}"),
    }
}

/// Fix hook --cwd for maw v2 bare repos.
///
/// Earlier versions of `detect_hook_paths` checked for `.jj` to identify bare repos,
/// which broke after the migration to Git+manifold. This re-registers hooks that have
/// `--cwd .../ws/default` with `--cwd .../` (the repo root) instead.
fn migrate_hook_cwd(fx: &Effects, config: &Config, project_root: &Path, allow_live_hooks: bool) {
    // Detect maw v2: project_root may be ws/default/ (inner sync) or the bare root
    let bare_root = if project_root.ends_with("ws/default") {
        project_root.parent().and_then(Path::parent)
    } else if project_root.join(".manifold").exists() {
        Some(project_root)
    } else {
        None
    };

    let bare_root = match bare_root {
        Some(r) if r.join(".manifold").exists() => r,
        _ => return,
    };

    let ws_default_str = bare_root.join("ws").join("default").display().to_string();
    let root_str = bare_root.display().to_string();

    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return;
    };

    let name = &config.project.name;
    let agent = config.default_agent();
    for hook in hooks {
        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        // Accept both current and legacy description prefixes
        let is_ours = desc.starts_with(&format!("edict:{name}:"))
            || desc.starts_with(&format!("botbox:{name}:"));
        if !is_ours {
            continue;
        }

        let Some(cmd) = hook.get("command").and_then(|c| c.as_array()) else {
            continue;
        };
        let cmd_strs: Vec<&str> = cmd.iter().filter_map(|v| v.as_str()).collect();

        // Check if any --cwd arg still points to ws/default
        let has_stale_cwd = cmd_strs
            .windows(2)
            .any(|w| w[0] == "--cwd" && w[1] == ws_default_str);
        if !has_stale_cwd {
            continue;
        }

        // Re-register with the correct cwd via the init helpers
        let Some(id) = hook.get("id").and_then(|i| i.as_str()) else {
            continue;
        };

        // Remove old hook first
        if fx
            .run(&Tool::new("rite").args(&["hooks", "remove", id]))
            .is_err()
        {
            continue;
        }

        let is_router = desc.ends_with(":responder");
        if is_router {
            let responder_ml = config
                .agents
                .responder
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            let _ = super::init::register_router_hook(
                fx,
                &root_str,
                &root_str,
                name,
                &agent,
                responder_ml,
                allow_live_hooks,
            );
            fx.announce(format!("  Fixed hook --cwd: {desc} → repo root"));
        } else {
            fx.announce(format!("  Retired stale reviewer hook {desc}"));
        }
    }
}

/// Migrate router hook claim pattern from `agent://{name}-router` to `agent://{name}-dev`
/// and spawn name from `{name}-router` to `{name}-responder`.
///
/// Earlier versions used a vestigial `-router` claim that nobody actually staked.
/// The new pattern uses `-dev` which matches the responder's own agent claim,
/// preventing re-trigger while processing.
fn migrate_router_hook_claim(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    allow_live_hooks: bool,
) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return;
    };

    let name = &config.project.name;
    let old_claim = format!("agent://{name}-router");

    for hook in hooks {
        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if desc != format!("edict:{name}:responder") && desc != format!("botbox:{name}:responder") {
            continue;
        }

        // Check if the hook still uses the old claim pattern
        let claim = hook
            .get("condition")
            .and_then(|c| c.get("pattern"))
            .and_then(|p| p.as_str())
            .unwrap_or("");
        if claim != old_claim {
            continue;
        }

        let Some(id) = hook.get("id").and_then(|i| i.as_str()) else {
            continue;
        };

        // Remove old hook and re-register with new claim pattern
        if fx
            .run(&Tool::new("rite").args(&["hooks", "remove", id]))
            .is_err()
        {
            continue;
        }

        let agent = config.default_agent();
        // Resolve hook paths the same way migrate_hook_cwd does
        let bare_root = if project_root.ends_with("ws/default") {
            project_root
                .parent()
                .and_then(Path::parent)
                .filter(|r| r.join(".manifold").exists())
        } else if project_root.join(".manifold").exists() {
            Some(project_root)
        } else {
            None
        };
        let root_str = bare_root.map_or_else(
            || project_root.display().to_string(),
            |r| r.display().to_string(),
        );
        let responder_ml = config
            .agents
            .responder
            .as_ref()
            .and_then(|r| r.memory_limit.as_deref());
        let _ = super::init::register_router_hook(
            fx,
            &root_str,
            &root_str,
            name,
            &agent,
            responder_ml,
            allow_live_hooks,
        );
        fx.announce(format!(
            "  Migrated router hook claim: agent://{name}-router → agent://{name}-dev"
        ));
    }
}

/// Migrate botty → vessel: update config key on disk and re-register rite hooks.
///
/// Idempotent — skips steps already done.
fn migrate_vessel_hooks(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    config_path: &Path,
    allow_live_hooks: bool,
) {
    // 1. Update config TOML on disk: botty → vessel, crit → seal, botbus → rite
    if let Ok(content) = fx.read_to_string(config_path) {
        let mut updated = content;
        let mut changed = false;

        if updated.contains("botty = ") {
            updated = updated.replace("botty = ", "vessel = ");
            changed = true;
            fx.announce("Migrated config: tools.botty → tools.vessel");
        }
        if updated.contains("crit = ") {
            updated = updated.replace("crit = ", "seal = ");
            changed = true;
            fx.announce("Migrated config: tools.crit → tools.seal");
        }
        if updated.contains("botbus = ") {
            updated = updated.replace("botbus = ", "rite = ");
            changed = true;
            fx.announce("Migrated config: tools.botbus → tools.rite");
        }

        if changed && let Err(e) = fx.write(config_path, updated) {
            tracing::warn!("failed to update config tool renames: {e}");
        }
    }

    // 2. Re-register edict hooks that still call `botty spawn` with `vessel spawn`.
    //    ensure_rite_hook deduplicates by description, so calling register_*_hook
    //    will remove the old hook and re-add it with the updated command.
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let hooks = match parsed.get("hooks").and_then(|h| h.as_array()) {
        Some(h) => h.clone(),
        None => return,
    };

    let name = &config.project.name;

    // Resolve root path (same logic as other hook migrations)
    let bare_root = if project_root.ends_with("ws/default") {
        project_root
            .parent()
            .and_then(Path::parent)
            .filter(|r| r.join(".manifold").exists())
    } else if project_root.join(".manifold").exists() {
        Some(project_root)
    } else {
        None
    };
    let root_str = bare_root.map_or_else(
        || project_root.display().to_string(),
        |r| r.display().to_string(),
    );
    let agent = config.default_agent();

    for hook in &hooks {
        // Only migrate hooks whose command array contains "botty"
        let uses_botty = hook
            .get("command")
            .and_then(|c| c.as_array())
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("botty")));
        if !uses_botty {
            continue;
        }

        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");

        if desc == format!("edict:{name}:responder") {
            let ml = config
                .agents
                .responder
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            let _ = super::init::register_router_hook(
                fx,
                &root_str,
                &root_str,
                name,
                &agent,
                ml,
                allow_live_hooks,
            );
            fx.announce("  Migrated router hook: vessel spawn (was botty)");
        } else if let Some(role) = desc
            .strip_prefix(&format!("edict:{name}:reviewer-"))
            .filter(|r| !r.is_empty())
        {
            fx.announce(format!("  Reviewer hook {role} will be retired"));
        }
    }
}

/// Report whether this project should have a router (responder) hook.
///
/// The responder is the single entrypoint for project channel messages, and it
/// runs entirely on rite. A project with rite disabled has no channel to route.
const fn router_hook_enabled(config: &Config) -> bool {
    config.tools.rite
}

/// Ensure the router (responder) hook exists.
///
/// `edict init` registers it, but a project can end up without one:
///
/// - the hook was removed by hand or lost from the hook store, or
/// - the project was renamed, so the old hook carries the old description
///   (`edict:<old-name>:responder`) and no migration matches it. Once that stale
///   hook goes away, nothing recreates it.
///
/// Sync already guarantees this for reviewer hooks. Without the same guarantee
/// for the router hook, `edict sync` reports success on a project whose channel
/// answers nobody.
fn ensure_router_hook(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    allow_live_hooks: bool,
) -> Result<()> {
    if !router_hook_enabled(config) {
        return Ok(());
    }

    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return Ok(()),
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return Ok(());
    };

    let name = &config.project.name;
    let description = format!("edict:{name}:responder");

    let exists = hooks.iter().any(|h| {
        h.get("description")
            .and_then(|d| d.as_str())
            .is_some_and(|d| d == description)
    });
    if exists {
        return Ok(());
    }

    let root_str = resolve_hook_root(project_root);
    let agent = config.default_agent();
    let ml = config
        .agents
        .responder
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    // register_router_hook listens on `name`, so report that, not
    // `config.channel()` — the two differ on projects renamed since init.
    //
    // Unlike the other migrations in this file, this one propagates: a
    // guard refusal here means the project's channel would silently answer
    // nobody, so `edict sync` should fail loudly rather than report success.
    super::init::register_router_hook(
        fx,
        &root_str,
        &root_str,
        name,
        &agent,
        ml,
        allow_live_hooks,
    )?;
    fx.announce(format!("  Registered missing router hook for #{name}"));
    Ok(())
}

/// Migrate hooks that still use BOTBUS_* env-inherit vars to RITE_*.
///
/// These hooks have correct `edict:` descriptions but were registered before
/// rite was renamed from botbus, so their `--env-inherit` still references
/// `BOTBUS_CHANNEL`, `BOTBUS_MESSAGE_ID`, etc.
fn migrate_botbus_env_hooks(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    allow_live_hooks: bool,
) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let hooks = match parsed.get("hooks").and_then(|h| h.as_array()) {
        Some(h) => h.clone(),
        None => return,
    };

    let name = &config.project.name;

    let bare_root = if project_root.ends_with("ws/default") {
        project_root
            .parent()
            .and_then(Path::parent)
            .filter(|r| r.join(".manifold").exists())
    } else if project_root.join(".manifold").exists() {
        Some(project_root)
    } else {
        None
    };
    let root_str = bare_root.map_or_else(
        || project_root.display().to_string(),
        |r| r.display().to_string(),
    );
    let agent = config.default_agent();

    for hook in &hooks {
        // Only look at hooks that have BOTBUS_ in their env-inherit
        let has_botbus_env = hook
            .get("command")
            .and_then(|c| c.as_array())
            .is_some_and(|arr| {
                arr.iter()
                    .any(|v| v.as_str().is_some_and(|s| s.contains("BOTBUS_")))
            });
        if !has_botbus_env {
            continue;
        }

        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");

        // Only migrate hooks for this project
        if desc == format!("edict:{name}:responder") {
            let ml = config
                .agents
                .responder
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            let _ = super::init::register_router_hook(
                fx,
                &root_str,
                &root_str,
                name,
                &agent,
                ml,
                allow_live_hooks,
            );
            fx.announce("  Migrated router hook: RITE_* env vars (was BOTBUS_*)");
        } else if let Some(role) = desc
            .strip_prefix(&format!("edict:{name}:reviewer-"))
            .filter(|r| !r.is_empty())
        {
            fx.announce(format!("  Reviewer hook {role} will be retired"));
        }
    }
}

/// Converge hooks whose `--env-inherit` differs from the one edict registers now.
///
/// A spawned agent can only anchor its reply when the hook forwards the id of
/// the message that woke it, so an older list without `RITE_BATCH_MESSAGE_IDS`
/// leaves a lease batch with no anchor. Comparing against the current value
/// rather than probing for one marker also covers the move to the `RITE_*`
/// namespace, and any list edict adopts later.
///
/// Idempotent — a hook already carrying the current list is skipped.
fn migrate_hook_reply_env(
    fx: &Effects,
    config: &Config,
    project_root: &Path,
    allow_live_hooks: bool,
) {
    let output = match Tool::new("rite")
        .args(&["hooks", "list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return,
    };

    let parsed: serde_json::Value = match serde_json::from_str(&output.stdout) {
        Ok(v) => v,
        Err(_) => return,
    };

    let hooks = match parsed.get("hooks").and_then(|h| h.as_array()) {
        Some(h) => h.clone(),
        None => return,
    };

    let name = &config.project.name;
    let root_str = resolve_hook_root(project_root);
    let agent = config.default_agent();

    let wanted = crate::reply::hook_env_inherit();

    for hook in &hooks {
        // Only hooks that inherit an env list, and only when it is not the one
        // edict registers today.
        let command = hook.get("command").and_then(|c| c.as_array());
        let Some(command) = command else { continue };
        let inherits_env = command
            .iter()
            .any(|v| v.as_str().is_some_and(|s| s.contains("RITE_")));
        let current = command.iter().any(|v| v.as_str() == Some(wanted));
        if !inherits_env || current {
            continue;
        }

        let desc = hook
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");

        if desc == format!("edict:{name}:responder") {
            let ml = config
                .agents
                .responder
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            let _ = super::init::register_router_hook(
                fx,
                &root_str,
                &root_str,
                name,
                &agent,
                ml,
                allow_live_hooks,
            );
            fx.announce("  Converged router hook --env-inherit");
        } else if let Some(role) = desc
            .strip_prefix(&format!("edict:{name}:reviewer-"))
            .filter(|r| !r.is_empty())
        {
            fx.announce(format!("  Reviewer hook {role} will be retired"));
        }
    }
}

/// Resolve the path hooks should use as `--cwd` (the bare root when there is one).
fn resolve_hook_root(project_root: &Path) -> String {
    let bare_root = if project_root.ends_with("ws/default") {
        project_root
            .parent()
            .and_then(Path::parent)
            .filter(|r| r.join(".manifold").exists())
    } else if project_root.join(".manifold").exists() {
        Some(project_root)
    } else {
        None
    };
    bare_root.map_or_else(
        || project_root.display().to_string(),
        |r| r.display().to_string(),
    )
}

/// Migrate beads → bones: config key, data directory, .maw.toml, .sealignore, .gitignore.
///
/// This is idempotent — checks each step before acting.
fn migrate_beads_to_bones(fx: &Effects, project_root: &Path, config_path: &Path) -> Result<()> {
    let beads_dir = project_root.join(".beads");
    let bones_dir = project_root.join(".bones");

    // 1. If config has `tools.beads` (in TOML), rename to `tools.bones`
    //    The serde alias handles deserialization, but we want the file itself updated.
    if config_path.exists() {
        let content = fx.read_to_string(config_path)?;
        if content.contains("beads") && !content.contains("bones") {
            let updated = content.replace("beads = ", "bones = ");
            fx.write(config_path, updated)?;
            fx.announce("Migrated config: tools.beads → tools.bones");
        }
    }

    // 2. If .beads/ exists and .bones/ doesn't → run `bn init` + migrate data
    if beads_dir.exists() && !bones_dir.exists() {
        let beads_db = beads_dir.join("beads.db");
        // Initialize bones first
        match fx.run_command("bn", &["init"], Some(project_root)) {
            Ok(_) => fx.announce("Initialized bones"),
            Err(e) => tracing::warn!("bn init failed: {e}"),
        }
        // Migrate data if beads.db exists
        if beads_db.exists() {
            let db_path = beads_db.to_string_lossy().to_string();
            match fx.run_command(
                "bn",
                &["data", "migrate-from-beads", "--beads-db", &db_path],
                Some(project_root),
            ) {
                Ok(_) => fx.announce("Migrated beads data to bones"),
                Err(e) => tracing::warn!("beads data migration failed: {e}"),
            }
        }
    }

    // 3. Update .maw.toml: remove .beads/** entry (set auto_resolve_from_main to empty)
    let maw_toml = project_root.join(".maw.toml");
    if maw_toml.exists() {
        let content = fx.read_to_string(&maw_toml)?;
        if content.contains(".beads/") {
            // Remove the .beads/** line and set to empty array if it was the only entry
            let updated = content
                .lines()
                .filter(|line| !line.contains(".beads/"))
                .collect::<Vec<_>>()
                .join("\n");
            // If the array is now effectively empty, replace with empty
            let updated = updated.replace(
                "auto_resolve_from_main = [\n]",
                "auto_resolve_from_main = []",
            );
            fx.write(&maw_toml, format!("{updated}\n"))?;
            fx.announce("Updated .maw.toml: removed .beads/** entry");
        }
    }

    // 4. Update .sealignore: remove .beads/ line (bones handles its own sealignore)
    let sealignore = project_root.join(".sealignore");
    if sealignore.exists() {
        let content = fx.read_to_string(&sealignore)?;
        if content.contains(".beads/") {
            let updated: String = content
                .lines()
                .filter(|line| line.trim() != ".beads/")
                .collect::<Vec<_>>()
                .join("\n");
            let updated = if content.ends_with('\n') {
                format!("{updated}\n")
            } else {
                updated
            };
            fx.write(&sealignore, updated)?;
            fx.announce("Updated .sealignore: removed .beads/ entry");
        }
    }

    // 5. Update .gitignore: remove .bv/ line (bones is tracked, not ignored)
    let gitignore = project_root.join(".gitignore");
    if gitignore.exists() {
        let content = fx.read_to_string(&gitignore)?;
        if content.contains(".bv/") {
            let updated: String = content
                .lines()
                .filter(|line| line.trim() != ".bv/")
                .collect::<Vec<_>>()
                .join("\n");
            // Preserve trailing newline if original had one
            let updated = if content.ends_with('\n') {
                format!("{updated}\n")
            } else {
                updated
            };
            fx.write(&gitignore, updated)?;
            fx.announce("Updated .gitignore: removed .bv/ entry");
        }
    }

    Ok(())
}

/// Version control system detected in a project.
#[derive(Debug, PartialEq, Eq)]
enum Vcs {
    Git,
    None,
}

/// Detect which VCS manages this project root.
/// Looks for a `.git` file (worktree/maw) or directory at `project_root` or
/// any ancestor.
fn detect_vcs(project_root: &Path) -> Vcs {
    if project_root.ancestors().any(|p| p.join(".git").exists()) {
        return Vcs::Git;
    }
    Vcs::None
}

/// Compute SHA-256 hash of all workflow docs for the given layout.
///
/// The layout is mixed into the hash so that a project changing its on-disk
/// layout (e.g. a maw bare → root migration) invalidates the stored `.version`
/// and triggers a re-render of the workflow docs — even though the embedded
/// template source is unchanged.
#[must_use]
pub fn compute_docs_version(layout: Layout) -> String {
    let mut hasher = Sha256::new();
    hasher.update(layout.trunk_path().as_bytes());
    for (name, content) in WORKFLOW_DOCS {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    format!("{:x}", hasher.finalize())[..32].to_string()
}

/// Compute SHA-256 hash of all design docs
fn compute_design_docs_version() -> String {
    let mut hasher = Sha256::new();
    for (name, content) in DESIGN_DOCS {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    format!("{:x}", hasher.finalize())[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_hashes() {
        let docs_ver = compute_docs_version(Layout::Bare);
        assert_eq!(docs_ver.len(), 32);
        assert!(docs_ver.chars().all(|c| c.is_ascii_hexdigit()));

        let design_ver = compute_design_docs_version();
        assert_eq!(design_ver.len(), 32);
        assert!(design_ver.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn router_hook_follows_the_rite_tool_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".edict.toml");

        fs::write(
            &path,
            "version = \"1.0.16\"\n[project]\nname = \"demo\"\n\n[tools]\nrite = true\n",
        )
        .unwrap();
        assert!(router_hook_enabled(&Config::load(&path).unwrap()));

        fs::write(
            &path,
            "version = \"1.0.16\"\n[project]\nname = \"demo\"\n\n[tools]\nrite = false\n",
        )
        .unwrap();
        assert!(
            !router_hook_enabled(&Config::load(&path).unwrap()),
            "a project without rite has no channel to route"
        );
    }

    #[test]
    fn docs_version_differs_by_layout() {
        // A layout change must invalidate the docs version so that a maw
        // bare -> root migration re-renders the workflow docs on next sync.
        assert_ne!(
            compute_docs_version(Layout::Bare),
            compute_docs_version(Layout::Root),
        );
    }

    #[test]
    fn retired_model_migration_is_scoped_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".edict.toml");
        fs::write(
            &config_path,
            r#"version = "1.0.16"
[project]
name = "test"

[models]
fast = ["openai-codex/gpt-5.3-codex-spark:low", "anthropic/claude-opus-4-6:low"]
balanced = ["anthropic/claude-sonnet-4-6:medium", "openai-codex/gpt-5.3-codex:medium", "openai-codex/gpt-5.4:medium", "custom/model"]
strong = ["anthropic/claude-opus-4-6:high", "openai-codex/gpt-5.3-codex:xhigh"]
"#,
        )
        .unwrap();

        assert!(migrate_retired_model_defaults(&Effects::apply(), &config_path).unwrap());
        assert!(!migrate_retired_model_defaults(&Effects::apply(), &config_path).unwrap());

        let migrated = fs::read_to_string(config_path).unwrap();
        assert!(migrated.contains("openai-codex/gpt-5.6-sol"));
        assert!(migrated.contains("openai-codex/gpt-5.6-terra"));
        assert!(migrated.contains("openai-codex/gpt-5.6-luna"));
        assert!(migrated.contains("anthropic/claude-opus-4-8:high"));
        assert!(migrated.contains("anthropic/claude-sonnet-5:medium"));
        assert!(migrated.contains("anthropic/claude-opus-4-6:low"));
        assert!(migrated.contains("custom/model"));
        assert!(!migrated.contains("openai-codex/gpt-5.3-codex-spark:low"));
        assert!(!migrated.contains("openai-codex/gpt-5.3-codex:medium"));
        assert!(!migrated.contains("openai-codex/gpt-5.4:medium"));
        assert!(!migrated.contains("anthropic/claude-opus-4-6:high"));
        assert!(!migrated.contains("anthropic/claude-sonnet-4-6:medium"));
        assert!(!migrated.contains("openai-codex/gpt-5.3-codex:xhigh"));
    }

    #[test]
    fn test_workflow_docs_embedded() {
        assert!(!WORKFLOW_DOCS.is_empty());
        for (name, content) in WORKFLOW_DOCS {
            assert!(!name.is_empty());
            assert!(!content.is_empty());
        }
    }

    #[test]
    fn test_design_docs_embedded() {
        assert!(!DESIGN_DOCS.is_empty());
        for (name, content) in DESIGN_DOCS {
            assert!(!name.is_empty());
            assert!(!content.is_empty());
        }
    }

    #[test]
    fn retired_reviewer_config_is_scoped_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".edict.toml");
        fs::write(
            &config_path,
            r#"[agents.worker]
model = "fast"

[agents.reviewer]
model = "strong"
timeout = 900

[review]
reviewers = ["security"]
"#,
        )
        .unwrap();

        assert!(migrate_retired_reviewer_config(&Effects::apply(), &config_path).unwrap());
        assert!(!migrate_retired_reviewer_config(&Effects::apply(), &config_path).unwrap());
        let migrated = fs::read_to_string(&config_path).unwrap();
        assert!(!migrated.contains("[agents.reviewer]"));
        assert!(migrated.contains("[agents.worker]"));
        assert!(migrated.contains("reviewers = [\"security\"]"));
    }

    #[test]
    fn only_named_edict_reviewer_hooks_are_retired() {
        let edict_hook = serde_json::json!({
            "owner": "edict",
            "name": "edict:demo:reviewer-security",
        });
        let user_hook = serde_json::json!({
            "owner": "user",
            "name": "edict:demo:reviewer-security",
        });
        let unnamed_hook = serde_json::json!({
            "owner": "edict",
            "description": "edict:demo:reviewer-security",
        });
        let other_project = serde_json::json!({
            "owner": "edict",
            "name": "edict:other:reviewer-security",
        });

        assert!(is_owned_reviewer_hook(&edict_hook, "demo"));
        assert!(!is_owned_reviewer_hook(&user_hook, "demo"));
        assert!(!is_owned_reviewer_hook(&unnamed_hook, "demo"));
        assert!(!is_owned_reviewer_hook(&other_project, "demo"));
    }
}
