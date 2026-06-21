use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::error::ExitError;
use crate::layout::Layout;
use crate::subprocess::{Tool, run_command};
use crate::template::{TemplateContext, render_workflow_doc, update_managed_section};

#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Project root directory
    #[arg(long)]
    pub project_root: Option<PathBuf>,
    /// Check mode: exit non-zero if anything is stale, without making changes
    #[arg(long)]
    pub check: bool,
    /// Disable auto-commit (default: enabled)
    #[arg(long)]
    pub no_commit: bool,
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
        "review-loop.md",
        include_str!("../templates/docs/review-loop.md"),
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

/// Embedded reviewer prompts
pub const REVIEWER_PROMPTS: &[(&str, &str)] = &[
    (
        "reviewer.md",
        include_str!("../templates/reviewer.md.jinja"),
    ),
    (
        "reviewer-security.md",
        include_str!("../templates/reviewer-security.md.jinja"),
    ),
];

impl SyncArgs {
    /// Run the sync command: detect staleness and update managed files, hooks, and docs.
    ///
    /// # Errors
    ///
    /// Returns `Err` if config loading, file I/O, or a subprocess invocation fails,
    /// or if `--check` is set and components are out of sync.
    ///
    /// # Panics
    ///
    /// Panics if the current working directory cannot be determined.
    pub fn execute(&self) -> Result<()> {
        let project_root = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("Failed to get current dir"));

        // Detect maw v2 bare repo
        if crate::config::find_config(&project_root.join("ws/default")).is_some() {
            return self.handle_bare_repo(&project_root);
        }

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
                    fs::write(&toml_path, &toml_content)?;
                    fs::remove_file(&json_path)?;
                    println!("Migrated .botbox.json -> .edict.toml");
                }
                Err(e) => {
                    tracing::warn!("failed to migrate .botbox.json to .edict.toml: {e}");
                }
            }
        }

        // Migrate .botbox.toml -> .edict.toml (botbox era → edict era)
        let legacy_toml_path = project_root.join(crate::config::CONFIG_TOML_LEGACY);
        if legacy_toml_path.exists() && !toml_path.exists() {
            match fs::rename(&legacy_toml_path, &toml_path) {
                Ok(()) => println!("Migrated .botbox.toml -> .edict.toml"),
                Err(e) => tracing::warn!("failed to rename .botbox.toml to .edict.toml: {e}"),
            }
        }

        // Migrate .agents/botbox/ -> .agents/edict/ (botbox era → edict era)
        if agents_dir_legacy.exists() && !agents_dir_edict.exists() {
            match fs::rename(&agents_dir_legacy, &agents_dir_edict) {
                Ok(()) => println!("Migrated .agents/botbox/ -> .agents/edict/"),
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
        let prompts_stale = Self::check_prompts_staleness(&agents_dir)?;
        let design_docs_stale = Self::check_design_docs_staleness(&agents_dir)?;

        let any_stale = docs_stale || managed_stale || prompts_stale || design_docs_stale;

        if self.check {
            if any_stale {
                let mut parts = Vec::new();
                if docs_stale {
                    parts.push("workflow docs");
                }
                if managed_stale {
                    parts.push("AGENTS.md managed section");
                }
                if prompts_stale {
                    parts.push("reviewer prompts");
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
        self.cleanup_per_repo_hooks(&project_root)?;

        // Perform updates
        let mut changed_files = Vec::new();

        if docs_stale {
            Self::sync_workflow_docs(&agents_dir, layout)?;
            changed_files.push(".agents/edict/*.md");
            println!("Updated workflow docs");
        }

        if managed_stale {
            Self::sync_managed_section(&project_root, &config, layout)?;
            changed_files.push("AGENTS.md");
            println!("Updated AGENTS.md managed section");
        }

        if prompts_stale {
            Self::sync_prompts(&agents_dir)?;
            changed_files.push(".agents/edict/prompts/*.md");
            println!("Updated reviewer prompts");
        }

        if design_docs_stale {
            Self::sync_design_docs(&agents_dir)?;
            changed_files.push(".agents/edict/design/*.md");
            println!("Updated design docs");
        }

        // Clean up legacy JS artifacts (scripts, shell hooks)
        self.cleanup_legacy_artifacts(&agents_dir, &mut changed_files);

        // Migrate rite hooks from bun .mjs to edict run
        migrate_rite_hooks(&config);

        // Migrate rite hooks from botbox: descriptions to edict: descriptions
        migrate_botbox_rite_hooks_to_edict(&config, &project_root);

        // Fix hook --cwd for maw v2 (ws/default → repo root)
        migrate_hook_cwd(&config, &project_root);

        // Migrate router hook claim from agent://{name}-router → agent://{name}-dev
        migrate_router_hook_claim(&config, &project_root);

        // Migrate botty → vessel (config key + rite hooks)
        if !self.check {
            migrate_vessel_hooks(&config, &project_root, &config_path);
        }

        // Migrate BOTBUS_* → RITE_* env vars in hook commands
        if !self.check {
            migrate_botbus_env_hooks(&config, &project_root);
        }

        // Ensure reviewer mention hooks exist for declared reviewer roles
        // (independent of review.enabled — see ensure_reviewer_hooks)
        if !self.check {
            ensure_reviewer_hooks(&config, &project_root);
        }

        // Migrate beads → bones (config, data, tooling files)
        if !self.check {
            migrate_beads_to_bones(&project_root, &config_path)?;
        }

        // Auto-commit if changes were made
        if !changed_files.is_empty() && !self.no_commit {
            Self::auto_commit(&project_root, &changed_files)?;
        }

        println!("Sync complete");
        Ok(())
    }

    fn handle_bare_repo(&self, project_root: &Path) -> Result<()> {
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

        run_command("maw", &args, Some(&project_root))?;

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
                match fs::remove_file(&stale_path) {
                    Ok(()) => println!(
                        "Removed stale {stale_name} from bare repo root \
                         (authoritative config lives in ws/default/)"
                    ),
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
            fs::write(&stub_agents, stub_content)?;
            println!("Created bare-root AGENTS.md stub");
        }

        // Symlink .claude directory — use atomic approach to avoid TOCTOU
        let root_claude_dir = project_root.join(".claude");
        let ws_claude_dir = project_root.join("ws/default/.claude");

        if ws_claude_dir.exists() {
            // Check if already a correct symlink
            let needs_symlink = fs::read_link(&root_claude_dir)
                .map_or(true, |target| target != Path::new("ws/default/.claude"));

            if needs_symlink {
                // Use atomic rename pattern: create temp symlink, then rename over target
                let tmp_link = project_root.join(".claude.tmp");
                let _ = fs::remove_file(&tmp_link); // clean up any stale temp
                #[cfg(unix)]
                std::os::unix::fs::symlink("ws/default/.claude", &tmp_link)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir("ws/default/.claude", &tmp_link)?;

                // Atomic rename (on same filesystem)
                if let Err(e) = fs::rename(&tmp_link, &root_claude_dir) {
                    let _ = fs::remove_file(&tmp_link);
                    return Err(e).context("creating .claude symlink");
                }
                println!("Symlinked .claude → ws/default/.claude");
            }
        }

        // Symlink .pi directory
        let root_pi_dir = project_root.join(".pi");
        let ws_pi_dir = project_root.join("ws/default/.pi");

        if ws_pi_dir.exists() {
            let needs_symlink = fs::read_link(&root_pi_dir)
                .map_or(true, |target| target != Path::new("ws/default/.pi"));

            if needs_symlink {
                let tmp_link = project_root.join(".pi.tmp");
                let _ = fs::remove_file(&tmp_link);
                #[cfg(unix)]
                std::os::unix::fs::symlink("ws/default/.pi", &tmp_link)?;
                #[cfg(windows)]
                std::os::windows::fs::symlink_dir("ws/default/.pi", &tmp_link)?;

                if let Err(e) = fs::rename(&tmp_link, &root_pi_dir) {
                    let _ = fs::remove_file(&tmp_link);
                    return Err(e).context("creating .pi symlink");
                }
                println!("Symlinked .pi → ws/default/.pi");
            }
        }

        Ok(())
    }

    /// Remove legacy JS-era artifacts that are no longer needed.
    /// The Rust rewrite builds loops into the binary, so .mjs scripts and
    /// shell hook wrappers are dead weight.
    fn cleanup_legacy_artifacts(&self, agents_dir: &Path, changed_files: &mut Vec<&str>) {
        // Remove .agents/botbox/scripts/ (JS loop scripts)
        let scripts_dir = agents_dir.join("scripts");
        if scripts_dir.is_dir() {
            if self.check {
                tracing::warn!("legacy scripts/ directory exists (will be removed on sync)");
            } else {
                match fs::remove_dir_all(&scripts_dir) {
                    Ok(()) => {
                        println!("Removed legacy scripts/ directory");
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
                match fs::remove_dir_all(&hooks_dir) {
                    Ok(()) => {
                        println!("Removed legacy hooks/ directory");
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
                let _ = fs::remove_file(&path);
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

    fn check_prompts_staleness(agents_dir: &Path) -> Result<bool> {
        let version_file = agents_dir.join("prompts/.prompts-version");
        let current = compute_prompts_version();

        if !version_file.exists() {
            return Ok(true);
        }

        let installed = fs::read_to_string(&version_file)?.trim().to_string();
        Ok(installed != current)
    }

    /// Clean up per-repo hooks that are now managed globally.
    /// Removes botbox hooks from per-repo .claude/settings.json and .pi/extensions/.
    fn cleanup_per_repo_hooks(&self, project_root: &Path) -> Result<()> {
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
                        fs::remove_file(&settings_path)?;
                        // Also remove .claude dir if empty
                        let claude_dir = project_root.join(".claude");
                        if claude_dir.exists() && fs::read_dir(&claude_dir)?.next().is_none() {
                            fs::remove_dir(&claude_dir)?;
                        }
                    } else {
                        fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
                    }
                    println!(
                        "Cleaned up per-repo botbox hooks from .claude/settings.json (now managed globally via `botbox hooks install`)"
                    );
                }
            }
        }

        // Clean up per-repo Pi extension
        let pi_ext = project_root.join(".pi/extensions/botbox-hooks.ts");
        if pi_ext.exists() {
            fs::remove_file(&pi_ext)?;
            // Clean up empty dirs
            let pi_ext_dir = project_root.join(".pi/extensions");
            if pi_ext_dir.exists() && fs::read_dir(&pi_ext_dir)?.next().is_none() {
                fs::remove_dir(&pi_ext_dir)?;
            }
            let pi_dir = project_root.join(".pi");
            if pi_dir.exists() && fs::read_dir(&pi_dir)?.next().is_none() {
                fs::remove_dir(&pi_dir)?;
            }
            println!(
                "Cleaned up per-repo Pi extension (now managed globally via `botbox hooks install`)"
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

    fn sync_workflow_docs(agents_dir: &Path, layout: Layout) -> Result<()> {
        for (name, content) in WORKFLOW_DOCS {
            let path = agents_dir.join(name);
            let rendered = render_workflow_doc(content, layout)
                .with_context(|| format!("Failed to render {name}"))?;
            fs::write(&path, rendered)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }

        let version = compute_docs_version(layout);
        fs::write(agents_dir.join(".version"), version)?;

        Ok(())
    }

    fn sync_managed_section(project_root: &Path, config: &Config, layout: Layout) -> Result<()> {
        let agents_md = project_root.join("AGENTS.md");
        if !agents_md.exists() {
            return Ok(()); // Skip if no AGENTS.md
        }

        let content = fs::read_to_string(&agents_md)?;
        let ctx = TemplateContext::from_config(config, layout);
        let updated = update_managed_section(&content, &ctx)?;

        fs::write(&agents_md, updated)?;
        Ok(())
    }

    fn sync_prompts(agents_dir: &Path) -> Result<()> {
        let prompts_dir = agents_dir.join("prompts");
        fs::create_dir_all(&prompts_dir)?;

        for (name, content) in REVIEWER_PROMPTS {
            let path = prompts_dir.join(name);
            fs::write(&path, content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }

        let version = compute_prompts_version();
        fs::write(prompts_dir.join(".prompts-version"), version)?;

        Ok(())
    }

    // sync_hooks removed — hooks are now installed globally via `botbox hooks install`

    fn sync_design_docs(agents_dir: &Path) -> Result<()> {
        let design_dir = agents_dir.join("design");
        fs::create_dir_all(&design_dir)?;

        for (name, content) in DESIGN_DOCS {
            let path = design_dir.join(name);
            fs::write(&path, content)
                .with_context(|| format!("Failed to write {}", path.display()))?;
        }

        let version = compute_design_docs_version();
        fs::write(design_dir.join(".design-docs-version"), version)?;

        Ok(())
    }

    fn auto_commit(project_root: &Path, changed_files: &[&str]) -> Result<()> {
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
                run_command("git", &args, Some(project_root))?;

                // Only commit if there are staged changes
                let status =
                    run_command("git", &["diff", "--cached", "--quiet"], Some(project_root));
                if status.is_err() {
                    // diff --cached --quiet exits 1 when there are staged changes
                    run_command("git", &["commit", "-m", &message], Some(project_root))?;
                }
            }
            Vcs::None => unreachable!(),
        }

        Ok(())
    }
}

/// Migrate rite hooks from `botbox:` descriptions to `edict:` descriptions.
///
/// Finds hooks with `botbox:{name}:responder` or `botbox:{name}:reviewer-*` descriptions,
/// removes them, and re-registers with `edict:` prefix and `edict run` commands.
/// Called during `edict sync` on projects that were previously set up with `botbox`.
fn migrate_botbox_rite_hooks_to_edict(config: &Config, project_root: &Path) {
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
        if Tool::new("rite")
            .args(&["hooks", "remove", id])
            .run()
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
            super::init::register_router_hook(&root_str, &root_str, name, &agent, responder_ml);
            println!("  Migrated hook {desc} → edict:{name}:responder");
        } else if let Some(role) = desc.strip_prefix(&format!("botbox:{name}:reviewer-")) {
            let reviewer_agent = format!("{name}-{role}");
            let reviewer_ml = config
                .agents
                .reviewer
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            super::init::register_reviewer_hook(
                &root_str,
                &root_str,
                name,
                &agent,
                &reviewer_agent,
                reviewer_ml,
            );
            println!("  Migrated hook {desc} → edict:{name}:reviewer-{role}");
        }
    }
}

/// Migrate rite hooks from legacy formats to current `edict run` commands with descriptions.
///
/// Lists all hooks for this project's channel, identifies legacy hooks
/// (bun-based, old naming, missing descriptions), removes them, and
/// re-registers via `ensure_rite_hook` with proper descriptions for
/// future idempotent management.
fn migrate_rite_hooks(config: &Config) {
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
        let is_reviewer = cmd_strs
            .iter()
            .any(|s| s.contains("reviewer-loop") || s.contains("reviewer-loop.mjs"));

        if !is_router && !is_reviewer {
            continue;
        }

        let spawn_cwd = cmd_strs
            .windows(2)
            .find(|w| w[0] == "--cwd")
            .map_or(".", |w| w[1]);

        // Remove old hook (ensure_rite_hook handles dedup by description,
        // but these legacy hooks have no description so we remove manually)
        let remove = Tool::new("rite").args(&["hooks", "remove", &id]).run();

        if remove.is_err() || !remove.as_ref().expect("remove is Ok here").success() {
            tracing::warn!(hook_id = %id, "failed to remove legacy hook");
            continue;
        }

        if is_router {
            reregister_legacy_router_hook(config, name, &agent, env_inherit, spawn_cwd, &id);
        } else if is_reviewer {
            let reviewer_agent = hook
                .get("condition")
                .and_then(|c| c.get("agent"))
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string();

            if reviewer_agent.is_empty() {
                tracing::warn!(hook_id = %id, "could not determine reviewer agent for hook");
                continue;
            }

            reregister_legacy_reviewer_hook(
                config,
                name,
                &agent,
                env_inherit,
                spawn_cwd,
                &id,
                &reviewer_agent,
            );
        }
    }
}

/// Re-register a legacy router hook with the current `edict run responder` command.
fn reregister_legacy_router_hook(
    config: &Config,
    name: &str,
    agent: &str,
    env_inherit: &str,
    spawn_cwd: &str,
    id: &str,
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

    match crate::subprocess::ensure_rite_hook(&description, &router_args) {
        Ok(_) => println!("  Migrated router hook {id} → edict run responder"),
        Err(e) => tracing::warn!("failed to re-register router hook: {e}"),
    }
}

/// Re-register a legacy reviewer hook with the current `edict run reviewer-loop` command.
fn reregister_legacy_reviewer_hook(
    config: &Config,
    name: &str,
    agent: &str,
    env_inherit: &str,
    spawn_cwd: &str,
    id: &str,
    reviewer_agent: &str,
) {
    let role = reviewer_agent
        .strip_prefix(&format!("{name}-"))
        .unwrap_or(reviewer_agent);
    let claim_uri = format!("agent://{reviewer_agent}");
    let description = format!("edict:{name}:reviewer-{role}");
    let reviewer_ml = config
        .agents
        .reviewer
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    let mut reviewer_args: Vec<&str> = vec![
        "--agent",
        agent,
        "--channel",
        name,
        "--mention",
        reviewer_agent,
        "--claim",
        &claim_uri,
        "--claim-owner",
        reviewer_agent,
        "--ttl",
        "600",
        "--priority",
        "1",
        "--cwd",
        spawn_cwd,
        "--",
        "vessel",
        "spawn",
        "--env-inherit",
        env_inherit,
    ];
    if let Some(limit) = reviewer_ml {
        reviewer_args.push("--memory-limit");
        reviewer_args.push(limit);
    }
    reviewer_args.extend_from_slice(&[
        "--name",
        reviewer_agent,
        "--cwd",
        spawn_cwd,
        "--",
        "edict",
        "run",
        "reviewer-loop",
        "--agent",
        reviewer_agent,
    ]);

    match crate::subprocess::ensure_rite_hook(&description, &reviewer_args) {
        Ok(_) => println!(
            "  Migrated reviewer hook {id} → edict run reviewer-loop --agent {reviewer_agent}"
        ),
        Err(e) => {
            tracing::warn!(agent = %reviewer_agent, "failed to re-register reviewer hook: {e}");
        }
    }
}

/// Fix hook --cwd for maw v2 bare repos.
///
/// Earlier versions of `detect_hook_paths` checked for `.jj` to identify bare repos,
/// which broke after the migration to Git+manifold. This re-registers hooks that have
/// `--cwd .../ws/default` with `--cwd .../` (the repo root) instead.
fn migrate_hook_cwd(config: &Config, project_root: &Path) {
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
    let reviewers: Vec<String> = config
        .review
        .reviewers
        .iter()
        .map(|r| format!("{name}-{r}"))
        .collect();

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
        if Tool::new("rite")
            .args(&["hooks", "remove", id])
            .run()
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
            super::init::register_router_hook(&root_str, &root_str, name, &agent, responder_ml);
            println!("  Fixed hook --cwd: {desc} → repo root");
        } else {
            let reviewer_ml = config
                .agents
                .reviewer
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            // Find which reviewer this is for
            for reviewer in &reviewers {
                if desc.contains(&reviewer.replace(&format!("{name}-"), "")) {
                    super::init::register_reviewer_hook(
                        &root_str,
                        &root_str,
                        name,
                        &agent,
                        reviewer,
                        reviewer_ml,
                    );
                    println!("  Fixed hook --cwd: {desc} → repo root");
                    break;
                }
            }
        }
    }
}

/// Migrate router hook claim pattern from `agent://{name}-router` to `agent://{name}-dev`
/// and spawn name from `{name}-router` to `{name}-responder`.
///
/// Earlier versions used a vestigial `-router` claim that nobody actually staked.
/// The new pattern uses `-dev` which matches the responder's own agent claim,
/// preventing re-trigger while processing.
fn migrate_router_hook_claim(config: &Config, project_root: &Path) {
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
        if Tool::new("rite")
            .args(&["hooks", "remove", id])
            .run()
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
        super::init::register_router_hook(&root_str, &root_str, name, &agent, responder_ml);
        println!("  Migrated router hook claim: agent://{name}-router → agent://{name}-dev");
    }
}

/// Migrate botty → vessel: update config key on disk and re-register rite hooks.
///
/// Idempotent — skips steps already done.
fn migrate_vessel_hooks(config: &Config, project_root: &Path, config_path: &Path) {
    // 1. Update config TOML on disk: botty → vessel, crit → seal, botbus → rite
    if let Ok(content) = fs::read_to_string(config_path) {
        let mut updated = content;
        let mut changed = false;

        if updated.contains("botty = ") {
            updated = updated.replace("botty = ", "vessel = ");
            changed = true;
            println!("Migrated config: tools.botty → tools.vessel");
        }
        if updated.contains("crit = ") {
            updated = updated.replace("crit = ", "seal = ");
            changed = true;
            println!("Migrated config: tools.crit → tools.seal");
        }
        if updated.contains("botbus = ") {
            updated = updated.replace("botbus = ", "rite = ");
            changed = true;
            println!("Migrated config: tools.botbus → tools.rite");
        }

        if changed && let Err(e) = fs::write(config_path, updated) {
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
            super::init::register_router_hook(&root_str, &root_str, name, &agent, ml);
            println!("  Migrated router hook: vessel spawn (was botty)");
        } else if let Some(role) = desc
            .strip_prefix(&format!("edict:{name}:reviewer-"))
            .filter(|r| !r.is_empty())
        {
            let reviewer_agent = format!("{name}-{role}");
            let ml = config
                .agents
                .reviewer
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            super::init::register_reviewer_hook(
                &root_str,
                &root_str,
                name,
                &agent,
                &reviewer_agent,
                ml,
            );
            println!("  Migrated reviewer hook {role}: vessel spawn (was botty)");
        }
    }
}

/// Ensure reviewer hooks exist when review is enabled in config.
///
/// `edict init` registers these, but they can be lost during migrations or
/// if a project was initialized before reviewer hooks were added. This checks
/// for each configured reviewer role and registers the hook if missing.
/// Reviewer roles that should have `@<project>-<role>` mention hooks registered.
///
/// Gated on seal being available and the role being *declared* in
/// `review.reviewers` — deliberately NOT on `review.enabled`. `review.enabled`
/// only governs whether the dev/worker loops auto-request a review and whether
/// finish/merge gate on approval; tagging `@<project>-<role>` in a one-off
/// message must spawn a reviewer even when auto-review is off.
fn reviewer_hook_roles(config: &Config) -> &[String] {
    if config.tools.seal {
        &config.review.reviewers
    } else {
        &[]
    }
}

fn ensure_reviewer_hooks(config: &Config, project_root: &Path) {
    let roles = reviewer_hook_roles(config);
    if roles.is_empty() {
        return;
    }

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
    let agent = config.default_agent();

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

    let ml = config
        .agents
        .reviewer
        .as_ref()
        .and_then(|r| r.memory_limit.as_deref());

    for role in roles {
        let description = format!("edict:{name}:reviewer-{role}");

        // Check if hook already exists
        let exists = hooks.iter().any(|h| {
            h.get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| d == description)
        });

        if !exists {
            let reviewer_agent = format!("{name}-{role}");
            super::init::register_reviewer_hook(
                &root_str,
                &root_str,
                name,
                &agent,
                &reviewer_agent,
                ml,
            );
            println!("  Registered missing reviewer hook: {role}");
        }
    }
}

/// Migrate hooks that still use BOTBUS_* env-inherit vars to RITE_*.
///
/// These hooks have correct `edict:` descriptions but were registered before
/// rite was renamed from botbus, so their `--env-inherit` still references
/// `BOTBUS_CHANNEL`, `BOTBUS_MESSAGE_ID`, etc.
fn migrate_botbus_env_hooks(config: &Config, project_root: &Path) {
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
            super::init::register_router_hook(&root_str, &root_str, name, &agent, ml);
            println!("  Migrated router hook: RITE_* env vars (was BOTBUS_*)");
        } else if let Some(role) = desc
            .strip_prefix(&format!("edict:{name}:reviewer-"))
            .filter(|r| !r.is_empty())
        {
            let reviewer_agent = format!("{name}-{role}");
            let ml = config
                .agents
                .reviewer
                .as_ref()
                .and_then(|r| r.memory_limit.as_deref());
            super::init::register_reviewer_hook(
                &root_str,
                &root_str,
                name,
                &agent,
                &reviewer_agent,
                ml,
            );
            println!("  Migrated reviewer hook {role}: RITE_* env vars (was BOTBUS_*)");
        }
    }
}

/// Migrate beads → bones: config key, data directory, .maw.toml, .sealignore, .gitignore.
///
/// This is idempotent — checks each step before acting.
fn migrate_beads_to_bones(project_root: &Path, config_path: &Path) -> Result<()> {
    let beads_dir = project_root.join(".beads");
    let bones_dir = project_root.join(".bones");

    // 1. If config has `tools.beads` (in TOML), rename to `tools.bones`
    //    The serde alias handles deserialization, but we want the file itself updated.
    if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        if content.contains("beads") && !content.contains("bones") {
            let updated = content.replace("beads = ", "bones = ");
            fs::write(config_path, updated)?;
            println!("Migrated config: tools.beads → tools.bones");
        }
    }

    // 2. If .beads/ exists and .bones/ doesn't → run `bn init` + migrate data
    if beads_dir.exists() && !bones_dir.exists() {
        let beads_db = beads_dir.join("beads.db");
        // Initialize bones first
        match run_command("bn", &["init"], Some(project_root)) {
            Ok(_) => println!("Initialized bones"),
            Err(e) => tracing::warn!("bn init failed: {e}"),
        }
        // Migrate data if beads.db exists
        if beads_db.exists() {
            let db_path = beads_db.to_string_lossy().to_string();
            match run_command(
                "bn",
                &["data", "migrate-from-beads", "--beads-db", &db_path],
                Some(project_root),
            ) {
                Ok(_) => println!("Migrated beads data to bones"),
                Err(e) => tracing::warn!("beads data migration failed: {e}"),
            }
        }
    }

    // 3. Update .maw.toml: remove .beads/** entry (set auto_resolve_from_main to empty)
    let maw_toml = project_root.join(".maw.toml");
    if maw_toml.exists() {
        let content = fs::read_to_string(&maw_toml)?;
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
            fs::write(&maw_toml, format!("{updated}\n"))?;
            println!("Updated .maw.toml: removed .beads/** entry");
        }
    }

    // 4. Update .sealignore: remove .beads/ line (bones handles its own sealignore)
    let sealignore = project_root.join(".sealignore");
    if sealignore.exists() {
        let content = fs::read_to_string(&sealignore)?;
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
            fs::write(&sealignore, updated)?;
            println!("Updated .sealignore: removed .beads/ entry");
        }
    }

    // 5. Update .gitignore: remove .bv/ line (bones is tracked, not ignored)
    let gitignore = project_root.join(".gitignore");
    if gitignore.exists() {
        let content = fs::read_to_string(&gitignore)?;
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
            fs::write(&gitignore, updated)?;
            println!("Updated .gitignore: removed .bv/ entry");
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

/// Compute SHA-256 hash of all reviewer prompts
fn compute_prompts_version() -> String {
    let mut hasher = Sha256::new();
    for (name, content) in REVIEWER_PROMPTS {
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

        let prompts_ver = compute_prompts_version();
        assert_eq!(prompts_ver.len(), 32);
        assert!(prompts_ver.chars().all(|c| c.is_ascii_hexdigit()));

        let design_ver = compute_design_docs_version();
        assert_eq!(design_ver.len(), 32);
        assert!(design_ver.chars().all(|c| c.is_ascii_hexdigit()));
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

    fn config_with(seal: bool, enabled: bool, reviewers: &[&str]) -> Config {
        let reviewer_lines = reviewers
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_str = format!(
            r#"
version = "1.0.0"

[project]
name = "test"

[tools]
seal = {seal}

[review]
enabled = {enabled}
reviewers = [{reviewer_lines}]
"#
        );
        Config::parse_toml(&toml_str).unwrap()
    }

    #[test]
    fn reviewer_hook_roles_ignore_review_enabled() {
        // A declared reviewer role gets a mention hook regardless of whether
        // auto-review (review.enabled) is on, so `@<project>-<role>` one-off
        // tags work even when the dev/worker loops never auto-request review.
        let off = config_with(true, false, &["security"]);
        let on = config_with(true, true, &["security"]);
        assert_eq!(reviewer_hook_roles(&off), &["security".to_string()]);
        assert_eq!(reviewer_hook_roles(&on), &["security".to_string()]);
    }

    #[test]
    fn reviewer_hook_roles_require_seal() {
        // reviewer-loop needs seal; without it, registering a mention hook would
        // spawn an agent that can't do anything.
        let no_seal = config_with(false, true, &["security"]);
        assert!(reviewer_hook_roles(&no_seal).is_empty());
    }

    #[test]
    fn reviewer_hook_roles_empty_without_declared_reviewers() {
        // No declared roles -> no hooks, even with seal + auto-review on.
        let none = config_with(true, true, &[]);
        assert!(reviewer_hook_roles(&none).is_empty());
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
    fn test_reviewer_prompts_embedded() {
        assert_eq!(REVIEWER_PROMPTS.len(), 2);
        assert!(REVIEWER_PROMPTS.iter().any(|(n, _)| *n == "reviewer.md"));
        assert!(
            REVIEWER_PROMPTS
                .iter()
                .any(|(n, _)| *n == "reviewer-security.md")
        );
    }
}
