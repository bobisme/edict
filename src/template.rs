//! Template rendering for docs, prompts, and AGENTS.md managed section.

use minijinja::Environment;
use serde::Serialize;

use crate::config::{Config, ReviewConfig, ToolsConfig};
use crate::layout::Layout;

const MANAGED_START: &str = "<!-- edict:managed-start -->";
const MANAGED_END: &str = "<!-- edict:managed-end -->";
/// Legacy markers from the botbox era — recognized on read, replaced with new markers on write.
const MANAGED_START_LEGACY: &str = "<!-- botbox:managed-start -->";
const MANAGED_END_LEGACY: &str = "<!-- botbox:managed-end -->";

const AGENTS_MANAGED_TEMPLATE: &str = include_str!("templates/agents-managed.md.jinja");

/// Context data passed to templates
#[derive(Debug, Serialize)]
pub struct TemplateContext {
    /// Project configuration
    pub project: ProjectInfo,
    /// Tools configuration
    pub tools: ToolsConfig,
    /// Review configuration
    pub review: ReviewConfig,
    /// Install command (optional, legacy — use `release_instructions` instead)
    pub install_command: Option<String>,
    /// Freeform release instructions block inserted into managed AGENTS.md
    pub release_instructions: Option<String>,
    /// Check command run before merging (optional)
    pub check_command: Option<String>,
    /// Workflow docs with descriptions
    pub workflow_docs: Vec<DocEntry>,
    /// Design docs with descriptions (filtered by project type)
    pub design_docs: Vec<DocEntry>,
    /// Layout-dependent paths and command prefixes (flattened into the context
    /// so templates can reference `bn`, `trunk_path`, `is_root_layout`, etc.).
    #[serde(flatten)]
    pub layout: LayoutVars,
}

/// Layout-dependent template variables shared by the managed-section template and
/// the workflow docs. See [`crate::layout::Layout`] for the underlying semantics.
#[derive(Debug, Serialize, Clone)]
pub struct LayoutVars {
    /// True for the new root layout (trunk == repo root).
    pub is_root_layout: bool,
    /// `bn` invocation against the trunk (`bn`, or `maw exec default -- bn`).
    pub bn: String,
    /// `seal` invocation against the trunk (`seal`, or `maw exec default -- seal`).
    pub seal_default: String,
    /// Trunk working-copy path (`.` or `ws/default`).
    pub trunk_path: String,
    /// Workspace path prefix with trailing slash (`.maw/workspaces/` or `ws/`).
    pub ws_prefix: String,
    /// Trunk command prefix incl. trailing space (empty or `maw exec default -- `).
    pub default_prefix: String,
}

impl LayoutVars {
    #[must_use]
    pub fn new(layout: Layout) -> Self {
        Self {
            is_root_layout: layout.is_root(),
            bn: layout.bn_cmd().to_string(),
            seal_default: layout.seal_default_cmd().to_string(),
            trunk_path: layout.trunk_path().to_string(),
            ws_prefix: layout.ws_prefix().to_string(),
            default_prefix: layout.default_prefix().to_string(),
        }
    }
}

/// Render a single workflow doc through minijinja with layout-dependent variables.
///
/// Workflow docs are plain Markdown that may contain `{{ bn }}`, `{{ ws_prefix }}`,
/// `{% if is_root_layout %}` and the other [`LayoutVars`] fields. Docs with no
/// directives render unchanged.
///
/// # Errors
///
/// Returns an error if the doc contains invalid jinja that fails to render.
pub fn render_workflow_doc(content: &str, layout: Layout) -> anyhow::Result<String> {
    let mut env = Environment::new();
    // Preserve the doc's trailing newline (minijinja strips it by default), so
    // bare rendering stays byte-identical to the docs that ship today.
    env.set_keep_trailing_newline(true);
    Ok(env.render_str(content, LayoutVars::new(layout))?)
}

#[derive(Debug, Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub project_type: Vec<String>,
    pub default_agent: Option<String>,
    pub channel: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DocEntry {
    pub name: String,
    pub description: String,
}

impl TemplateContext {
    /// Build template context from project config and detected layout.
    pub fn from_config(config: &Config, layout: Layout) -> Self {
        let workflow_docs = list_workflow_docs();
        let design_docs = list_design_docs(&config.project.project_type);

        Self {
            layout: LayoutVars::new(layout),
            project: ProjectInfo {
                name: config.project.name.clone(),
                project_type: config.project.project_type.clone(),
                default_agent: config.project.default_agent.clone(),
                channel: config.project.channel.clone(),
            },
            tools: config.tools.clone(),
            review: config.review.clone(),
            install_command: config.project.install_command.clone(),
            release_instructions: config
                .project
                .release_instructions
                .as_deref()
                .map(dedent_and_trim),
            check_command: config.project.check_command.clone(),
            workflow_docs,
            design_docs,
        }
    }
}

/// One-line descriptions for the workflow-doc index in the managed section.
/// Every entry must name a doc in `WORKFLOW_DOCS` (a test checks this).
const WORKFLOW_DOC_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "worker-loop.md",
        "Full worker cycle: resume, triage, start, work, review, finish",
    ),
    (
        "triage.md",
        "Find one actionable bone and groom along the way",
    ),
    ("start.md", "Claim a bone, create its workspace, announce"),
    ("update.md", "Change a bone's state and announce it"),
    (
        "review-request.md",
        "Request a review: commit first, review range, retarget before re-request",
    ),
    (
        "review-response.md",
        "Handle reviewer feedback; no code after the LGTM",
    ),
    (
        "security-review.md",
        "Launch one dedicated security review; who sends what",
    ),
    (
        "finish.md",
        "Close the bone, merge or hand off, release claims; conflict recovery",
    ),
    (
        "merge-check.md",
        "Merge a workspace: review log, clean check, merge gates, conflicts",
    ),
    (
        "cross-channel.md",
        "Rite threads, ask-and-wait, message shape, cross-project asks",
    ),
    ("report-issue.md", "Superseded by cross-channel.md"),
    ("planning.md", "Turn a spec or PRD into actionable bones"),
    ("scout.md", "Explore unfamiliar code before planning"),
    (
        "proposal.md",
        "Propose and validate a significant change before building it",
    ),
    ("groom.md", "Groom ready bones to improve backlog quality"),
    (
        "mission.md",
        "Missions: split a parent bone across parallel workers",
    ),
    (
        "coordination.md",
        "Coordinate with sibling workers inside a mission",
    ),
    (
        "preflight.md",
        "Validate toolchain health before multi-agent work",
    ),
];

/// List all workflow docs with descriptions
fn list_workflow_docs() -> Vec<DocEntry> {
    WORKFLOW_DOC_DESCRIPTIONS
        .iter()
        .map(|(name, description)| DocEntry {
            name: (*name).to_string(),
            description: (*description).to_string(),
        })
        .collect()
}

/// List design docs filtered by project types
fn list_design_docs(project_types: &[String]) -> Vec<DocEntry> {
    let mut docs = Vec::new();

    // cli-conventions is eligible for all project types
    if !project_types.is_empty() {
        docs.push(DocEntry {
            name: "cli-conventions.md".to_string(),
            description: "CLI tool design for humans, agents, and machines".to_string(),
        });
    }

    docs
}

/// Render the AGENTS.md managed section
///
/// # Errors
///
/// Returns an error if the managed-section template fails to load or render.
pub fn render_managed_section(ctx: &TemplateContext) -> anyhow::Result<String> {
    let mut env = Environment::new();
    env.add_template("agents-managed", AGENTS_MANAGED_TEMPLATE)?;

    let template = env.get_template("agents-managed")?;
    let rendered = template.render(ctx)?;

    Ok(rendered)
}

/// Render a complete AGENTS.md file for a new project
///
/// # Errors
///
/// Returns an error if the managed-section template fails to render.
pub fn render_agents_md(config: &Config, layout: Layout) -> anyhow::Result<String> {
    let ctx = TemplateContext::from_config(config, layout);

    let tool_list = config
        .tools
        .enabled_tools()
        .into_iter()
        .map(|t| format!("`{t}`"))
        .collect::<Vec<_>>()
        .join(", ");

    let reviewer_line = if config.review.reviewers.is_empty() {
        String::new()
    } else {
        format!("\nReviewer roles: {}", config.review.reviewers.join(", "))
    };

    let managed = render_managed_section(&ctx)?;

    Ok(format!(
        "# {}\n\nProject type: {}\nTools: {}{}\n\n<!-- Add project-specific context below: architecture, conventions, key files, etc. -->\n\n{}{}\n{}\n",
        config.project.name,
        config.project.project_type.join(", "),
        tool_list,
        reviewer_line,
        MANAGED_START,
        managed,
        MANAGED_END
    ))
}

/// Update the managed section in an existing AGENTS.md.
///
/// Handles both current (`edict:managed-*`) and legacy (`botbox:managed-*`) markers,
/// always writing back with current markers. This enables automatic migration of
/// AGENTS.md files from botbox-era projects on the next `edict sync`.
///
/// # Errors
///
/// Returns an error if the managed-section template fails to render.
pub fn update_managed_section(content: &str, ctx: &TemplateContext) -> anyhow::Result<String> {
    let managed = render_managed_section(ctx)?;
    let full_managed = format!("{MANAGED_START}\n{managed}\n{MANAGED_END}");

    // Try current markers first
    if let Some(start_idx) = content.find(MANAGED_START)
        && let Some(end_idx) = content.find(MANAGED_END)
        && end_idx > start_idx
    {
        let before = &content[..start_idx];
        let after = &content[end_idx + MANAGED_END.len()..];
        return Ok(format!("{before}{full_managed}{after}"));
    }

    // Try legacy markers (botbox era) — replace them with current markers
    if let Some(start_idx) = content.find(MANAGED_START_LEGACY)
        && let Some(end_idx) = content.find(MANAGED_END_LEGACY)
        && end_idx > start_idx
    {
        let before = &content[..start_idx];
        let after = &content[end_idx + MANAGED_END_LEGACY.len()..];
        return Ok(format!("{before}{full_managed}{after}"));
    }

    // Missing or invalid markers — strip any stale marker fragments and append
    let temp = content
        .replace(MANAGED_START, "")
        .replace(MANAGED_END, "")
        .replace(MANAGED_START_LEGACY, "")
        .replace(MANAGED_END_LEGACY, "");
    let cleaned = temp.trim_end();
    Ok(format!("{cleaned}\n\n{full_managed}\n"))
}

/// Dedent a multi-line string by stripping the common leading whitespace, then trim.
///
/// Handles TOML multi-line strings where indentation is relative to the config file.
fn dedent_and_trim(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    // Find minimum indentation among non-empty lines
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l.trim()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::sync::WORKFLOW_DOCS;

    /// Every workflow doc must render cleanly in both layouts with all jinja
    /// directives resolved, and must not leak the *other* layout's conventions.
    #[test]
    fn all_workflow_docs_render_in_both_layouts() {
        for (name, content) in WORKFLOW_DOCS {
            let bare = render_workflow_doc(content, Layout::Bare)
                .unwrap_or_else(|e| panic!("{name} failed to render (bare): {e}"));
            let root = render_workflow_doc(content, Layout::Root)
                .unwrap_or_else(|e| panic!("{name} failed to render (root): {e}"));

            // No unresolved jinja in either rendering.
            for (label, out) in [("bare", &bare), ("root", &root)] {
                assert!(
                    !out.contains("{{") && !out.contains("{%"),
                    "{name} ({label}) has unresolved jinja"
                );
            }

            // Root layout must not carry bare-only conventions, and vice versa.
            assert!(
                !root.contains("maw exec default -- bn"),
                "{name} (root) still prefixes bn with `maw exec default --`"
            );
            assert!(
                !root.contains("ws/$WS"),
                "{name} (root) still uses bare `ws/$WS` paths"
            );
            assert!(
                !bare.contains(".maw/workspaces"),
                "{name} (bare) leaked root-layout `.maw/workspaces` path"
            );
        }
    }

    /// Launch contracts in the workflow docs must be directly executable:
    /// `rite claims stake --ttl` takes whole seconds (no `m`/`h`/`d` unit
    /// suffix), and any `vessel spawn --cwd` must be an absolute path (or a
    /// command substitution that resolves to one) rather than a path that is
    /// relative to the vessel server's own working directory (bn-2c38).
    #[test]
    fn workflow_docs_have_no_unit_suffixed_ttl_or_relative_vessel_cwd() {
        let ttl_re = regex::Regex::new(r"--ttl[= ]+(\S+)").unwrap();
        let cwd_re = regex::Regex::new(r#"--cwd[= ]+"([^"]*)""#).unwrap();

        for (name, content) in WORKFLOW_DOCS {
            for layout in [Layout::Bare, Layout::Root] {
                let rendered = render_workflow_doc(content, layout)
                    .unwrap_or_else(|e| panic!("{name} failed to render ({layout:?}): {e}"));

                for cap in ttl_re.captures_iter(&rendered) {
                    let value = &cap[1];
                    assert!(
                        value.chars().all(|c| c.is_ascii_digit()),
                        "{name} ({layout:?}) has a unit-suffixed --ttl value: {value:?}; \
                         rite claims stake --ttl takes whole seconds"
                    );
                }

                for cap in cwd_re.captures_iter(&rendered) {
                    let value = &cap[1];
                    assert!(
                        value.starts_with('/') || value.starts_with("$("),
                        "{name} ({layout:?}) has a relative vessel --cwd: {value:?}; \
                         it must be absolute (it resolves against the vessel server's cwd, \
                         not the caller's)"
                    );
                }
            }
        }
    }

    /// Split a rendered doc into markdown sections (by `#`-headings *outside*
    /// fenced code blocks — a `#`-prefixed shell comment inside a fenced bash
    /// block is not a heading).
    fn markdown_sections(rendered: &str) -> Vec<String> {
        let mut sections = vec![String::new()];
        let mut in_fence = false;
        for line in rendered.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            } else if !in_fence
                && line.starts_with('#')
                && line.trim_start_matches('#').starts_with(' ')
            {
                sections.push(String::new());
            }
            let section = sections.last_mut().expect("at least one section");
            section.push_str(line);
            section.push('\n');
        }
        sections
    }

    /// A review that already exists is only ever moved forward by `seal
    /// reviews retarget` (bn-w912) — `seal reviews request` alone leaves its
    /// target commit pinned at the old anchor. Every workflow-doc mention of
    /// re-requesting review on an existing review must retarget it first, in
    /// the same section (initial `seal reviews create` calls are exempt: they
    /// have no prior target to move).
    #[test]
    fn workflow_docs_retarget_before_re_requesting_review() {
        for (name, content) in WORKFLOW_DOCS {
            for layout in [Layout::Bare, Layout::Root] {
                let rendered = render_workflow_doc(content, layout)
                    .unwrap_or_else(|e| panic!("{name} failed to render ({layout:?}): {e}"));

                for section in markdown_sections(&rendered) {
                    if section.contains("reviews request") {
                        assert!(
                            section.contains("reviews retarget"),
                            "{name} ({layout:?}) tells the agent to `seal reviews request` \
                             (re-request) without retargeting the review first in the same \
                             section:\n{section}"
                        );
                    }
                }
            }
        }
    }

    fn rendered_doc(name: &str, layout: Layout) -> String {
        let content = WORKFLOW_DOCS
            .iter()
            .find_map(|(n, content)| (*n == name).then_some(*content))
            .unwrap_or_else(|| panic!("{name} is embedded"));
        render_workflow_doc(content, layout).unwrap()
    }

    /// worker-loop.md must run `maw ws recover` before it recreates a missing
    /// or destroyed workspace (bn-30vn).
    #[test]
    fn worker_loop_doc_recovers_missing_workspace_first() {
        for layout in [Layout::Bare, Layout::Root] {
            let rendered = rendered_doc("worker-loop.md", layout);
            let mut found = 0;
            for line in rendered.lines() {
                let lower = line.to_lowercase();
                if lower.contains("workspace was destroyed")
                    || lower.contains("workspace is missing")
                {
                    found += 1;
                    assert!(
                        line.contains("maw ws recover <workspace>")
                            && line.contains("--to <new-name>"),
                        "worker-loop.md ({layout:?}) handles a missing workspace without \
                         `maw ws recover`:\n{line}"
                    );
                    let recover = line.find("maw ws recover").unwrap();
                    let scratch = line.find("from scratch").unwrap_or(usize::MAX);
                    assert!(recover < scratch, "recover must come before starting over");
                }
            }
            assert!(
                found >= 1,
                "worker-loop.md ({layout:?}) lost its missing-workspace path"
            );
        }
    }

    /// A worker merges reviewed work only through the protocol steps, and
    /// never pushes (bn-285u). The only hand-run `maw ws merge $WS ... --destroy`
    /// left in worker-loop.md and finish.md is the no-review fallback.
    #[test]
    fn worker_docs_route_reviewed_merge_through_protocol_and_never_push() {
        for name in ["worker-loop.md", "finish.md"] {
            for layout in [Layout::Bare, Layout::Root] {
                let rendered = rendered_doc(name, layout);
                assert!(
                    rendered.contains("edict protocol finish <bone-id> --agent $AGENT"),
                    "{name} ({layout:?}) must route the finish through edict protocol finish"
                );
                assert!(
                    rendered.contains("--no-merge"),
                    "{name} ({layout:?}) must keep the dispatched-worker --no-merge path"
                );
                for line in rendered.lines() {
                    if line.contains("maw ws merge $WS") && line.contains("--destroy") {
                        assert!(
                            line.contains("Without a review"),
                            "{name} ({layout:?}) has a hand-run merge outside the no-review \
                             fallback:\n{line}"
                        );
                        assert!(
                            !line.contains("unreviewed changes: stop"),
                            "{name} ({layout:?}) spells out the reviewed merge step instead \
                             of running the one the protocol prints:\n{line}"
                        );
                    }
                    assert!(
                        !line.contains("maw push"),
                        "{name} ({layout:?}) tells the worker to push:\n{line}"
                    );
                }
            }
        }
        let worker_loop = rendered_doc("worker-loop.md", Layout::Root);
        assert!(worker_loop.contains("seal review <review-id> --format json"));
        let subagent = worker_loop
            .find("Spawn a subagent to perform the review")
            .expect("subagent review path");
        let section_end = worker_loop[subagent..]
            .find("**STOP this iteration.**")
            .expect("subagent review path stops");
        assert!(
            worker_loop[subagent..subagent + section_end]
                .contains("seal review <review-id> --format json"),
            "the subagent review must be confirmed through the Seal verdict"
        );
    }

    #[test]
    fn security_review_contract_terminates_its_exact_vessel_session() {
        let content = WORKFLOW_DOCS
            .iter()
            .find_map(|(name, content)| (*name == "security-review.md").then_some(*content))
            .expect("security review workflow doc is embedded");
        let rendered = render_workflow_doc(content, Layout::Root).unwrap();

        assert!(rendered.contains("terminate_security_review_session()"));
        assert!(rendered.contains("vessel send-keys \"$session\" ctrl-c"));
        assert!(rendered.contains("vessel kill \"$session\""));
        assert!(rendered.contains("Do not send Rite messages, release $claim"));
        assert!(rendered.contains("releasing `$claim`"));

        let snapshot = rendered
            .find("Inspect `vessel snapshot \"$session\"`")
            .unwrap();
        let failure_teardown = rendered[snapshot..]
            .find("terminate_security_review_session`")
            .expect("failure path terminates the session");
        assert!(failure_teardown > 0, "failure snapshot precedes teardown");

        let success_verify = rendered
            .rfind("maw exec \"$ws\" -- seal review \"$review_id\" --format json")
            .unwrap();
        let success_teardown = rendered[success_verify..]
            .find("terminate_security_review_session ||")
            .expect("success path terminates before releasing the claim");
        let claim_release = rendered[success_verify..]
            .find("rite claims release --agent \"$reviewer\" \"$claim\"")
            .expect("success path releases the claim");
        assert!(success_teardown < claim_release);
    }

    /// Rendering each templated doc in this repo's own layout must be
    /// byte-identical to the committed `.agents/edict/*.md` — guaranteeing the
    /// layout templating produces exactly what `edict sync` ships here. This repo
    /// migrated bare -> root, so we render in whatever layout it currently uses
    /// (detected from the repo root) rather than assuming bare.
    #[test]
    fn render_matches_committed_docs() {
        use std::path::Path;
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let layout = Layout::detect(manifest);
        let dir = manifest.join(".agents/edict");
        let mut checked = 0;
        for (name, content) in WORKFLOW_DOCS {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let committed = std::fs::read_to_string(&path).unwrap();
            let rendered = render_workflow_doc(content, layout).unwrap();
            assert_eq!(
                rendered, committed,
                "{layout:?} render of {name} differs from committed .agents/edict/{name}"
            );
            checked += 1;
        }
        assert!(
            checked > 10,
            "expected to check >10 docs, checked {checked}"
        );
    }

    fn demo_config(tools: ToolsConfig, review_enabled: bool) -> Config {
        Config {
            version: "1.0.0".into(),
            project: crate::config::ProjectConfig {
                name: "demo".into(),
                project_type: vec!["cli".into()],
                default_agent: Some("demo-dev".into()),
                channel: Some("demo".into()),
                install_command: None,
                release_instructions: None,
                check_command: None,
                languages: vec![],
                critical_approvers: None,
            },
            tools,
            review: ReviewConfig {
                enabled: review_enabled,
                reviewers: vec![],
            },
            push_main: false,
            agents: crate::config::AgentsConfig::default(),
            models: crate::config::ModelsConfig::default(),
            env: std::collections::HashMap::default(),
        }
    }

    fn all_tools() -> ToolsConfig {
        ToolsConfig {
            bones: true,
            maw: true,
            seal: true,
            rite: true,
            vessel: true,
        }
    }

    fn managed(config: &Config, layout: Layout) -> String {
        render_managed_section(&TemplateContext::from_config(config, layout)).unwrap()
    }

    /// The bare-layout managed section must still carry the bare-only
    /// conventions (the trunk lives at `ws/default/`, bones go through
    /// `maw exec default --`), while the root rendering must not.
    #[test]
    fn managed_section_respects_layout() {
        let config = demo_config(all_tools(), true);

        let bare = managed(&config, Layout::Bare);
        assert!(bare.contains("bare repo"));
        assert!(bare.contains("Run bones through `maw exec default -- bn`"));
        assert!(bare.contains("Never edit `ws/default/` directly"));
        assert!(bare.contains("work in `ws/<bone-id>/`"));
        assert!(!bare.contains(".maw/workspaces"));

        let root = managed(&config, Layout::Root);
        assert!(root.contains(".maw/workspaces/<bone-id>/"));
        assert!(root.contains("Run `bn` directly at the repo root"));
        assert!(root.contains("Never edit the trunk at the repo root directly"));
        assert!(!root.contains("maw exec default -- bn"));
        assert!(!root.contains("bare repo"));
    }

    /// The managed section is tool pointers plus a short Rules list (bn-21zp).
    /// The cross-tool policy that no `--help` teaches must stay; the command
    /// tables and the false maw claims must not come back.
    #[test]
    fn managed_section_is_pointers_plus_rules() {
        let config = demo_config(all_tools(), true);
        for layout in [Layout::Bare, Layout::Root] {
            let out = managed(&config, layout);
            for pointer in [
                "`bn tldr`",
                "`maw tldr`",
                "`maw --help`",
                "`seal --help`",
                "`rite tldr`",
                "`edict protocol --help`",
                "the Rules win",
                "`<project>-dev`",
            ] {
                assert!(
                    out.contains(pointer),
                    "{layout:?}: missing pointer {pointer}"
                );
            }
            for rule in [
                "### Rules",
                "**Track all work in a bone.**",
                "`maw ws create <bone-id> --from main`",
                "never create git branches",
                "**Never merge or destroy `default`.**",
                "**Run the edict protocol at each transition:**",
                "only through `edict protocol merge <ws> --message",
                "Commit no code after the LGTM",
                "`maw ws resolve <ws> --list`",
                "**Run `maw ws recover` before you conclude work is lost**",
                "**Seal completion is not approval.**",
                "never mention `@<project>-security`",
                "**Answer `$RITE_MESSAGE_ID` with `--reply-to`.**",
                "`rite wait --reply-to <id> -t 300`",
                "never re-send",
                "leads with the bone id",
                "Workers do not push",
                "**Confirm before destructive actions**",
                "### Workflow Docs",
            ] {
                assert!(out.contains(rule), "{layout:?}: missing rule text {rule}");
            }
            for gone in [
                "Quick Reference",
                "| Operation | Command |",
                "### Bus Communication",
                "### Claims",
                "Simplified Technical English",
                "Replies to a human",
                "auto-sync",
                "handles branching",
                "--check` before `--destroy",
                "├──",
            ] {
                assert!(
                    !out.contains(gone),
                    "{layout:?}: dropped text came back: {gone}"
                );
            }
        }
    }

    /// Each tool toggle removes the bullets and pointers for that tool, and
    /// every toggle combination renders cleanly in both layouts (bn-21zp).
    #[test]
    fn managed_section_gates_bullets_on_tool_toggles() {
        type Disable = fn(&mut ToolsConfig);
        let on = managed(&demo_config(all_tools(), true), Layout::Root);
        let cases: [(&str, Disable, &[&str]); 4] = [
            (
                "bones",
                |t| t.bones = false,
                &["`bn tldr`", "Track all work in a bone", "Run `bn` directly"],
            ),
            (
                "maw",
                |t| t.maw = false,
                &[
                    "`maw tldr`",
                    "maw ws create",
                    "maw ws recover",
                    "maw ws resolve",
                    "Never merge or destroy",
                    "Layout:",
                ],
            ),
            (
                "seal",
                |t| t.seal = false,
                &[
                    "`seal --help`",
                    "Seal completion is not approval",
                    "Commit no code after the LGTM",
                    "maw exec <ws> -- seal",
                ],
            ),
            (
                "rite",
                |t| t.rite = false,
                &[
                    "`rite tldr`",
                    "$RITE_MESSAGE_ID",
                    "rite wait",
                    "labelled line",
                ],
            ),
        ];
        for (tool, disable, texts) in cases {
            let mut tools = all_tools();
            disable(&mut tools);
            let off = managed(&demo_config(tools, true), Layout::Root);
            for text in texts {
                assert!(on.contains(text), "all-on render lacks {text}");
                assert!(
                    !off.contains(text),
                    "{tool} disabled but the render still has {text}"
                );
            }
        }

        // Review disabled drops the Seal review bullets even with seal on.
        let no_review = managed(&demo_config(all_tools(), false), Layout::Root);
        assert!(!no_review.contains("Seal completion is not approval"));
        assert!(!no_review.contains("Commit no code after the LGTM"));
        assert!(no_review.contains("`seal --help`"));

        for bits in 0u8..32 {
            let tools = ToolsConfig {
                bones: bits & 1 != 0,
                maw: bits & 2 != 0,
                seal: bits & 4 != 0,
                rite: bits & 8 != 0,
                vessel: bits & 16 != 0,
            };
            for (review, extras) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut config = demo_config(tools.clone(), review);
                if extras {
                    config.project.check_command = Some("just check".into());
                    config.project.install_command = Some("just install".into());
                    config.project.release_instructions = Some("1. Tag\n2. Push".into());
                }
                for layout in [Layout::Bare, Layout::Root] {
                    let out = managed(&config, layout);
                    let what = format!("{tools:?} review={review} extras={extras} {layout:?}");
                    assert!(
                        !out.contains("{{") && !out.contains("{%"),
                        "unresolved jinja: {what}"
                    );
                    assert!(out.starts_with("## Edict Workflow\n"), "{what}");
                    assert!(out.contains("\n### Rules\n\n- "), "{what}");
                    assert!(!out.contains("\n\n\n"), "blank-line run: {what}");
                }
            }
        }
    }

    /// Every `.agents/edict/*.md` link in the managed section names a doc that
    /// `edict sync` ships, every `#anchor` is a heading in that doc, and every
    /// shipped doc is in the Workflow Docs index.
    #[test]
    fn managed_section_has_no_dangling_links() {
        let link_re =
            regex::Regex::new(r"\(\.agents/edict/([a-z-]+\.md)(?:#([a-z0-9-]+))?\)").unwrap();
        let slug = |heading: &str| -> String {
            heading
                .trim_start_matches('#')
                .trim()
                .to_lowercase()
                .chars()
                .filter_map(|c| match c {
                    'a'..='z' | '0'..='9' | '-' => Some(c),
                    ' ' => Some('-'),
                    _ => None,
                })
                .collect()
        };
        for layout in [Layout::Bare, Layout::Root] {
            let out = managed(&demo_config(all_tools(), true), layout);
            let mut linked = std::collections::HashSet::new();
            for cap in link_re.captures_iter(&out) {
                let name = &cap[1];
                linked.insert(name.to_string());
                let doc = WORKFLOW_DOCS
                    .iter()
                    .find_map(|(n, c)| (*n == name).then_some(*c))
                    .unwrap_or_else(|| panic!("managed section links a missing doc: {name}"));
                if let Some(anchor) = cap.get(2) {
                    let rendered = render_workflow_doc(doc, layout).unwrap();
                    let found = markdown_sections(&rendered)
                        .iter()
                        .filter_map(|s| s.lines().next())
                        .filter(|l| l.starts_with('#'))
                        .any(|l| slug(l) == anchor.as_str());
                    assert!(
                        found,
                        "{name}#{} is not a heading in {name}",
                        anchor.as_str()
                    );
                }
            }
            for (name, _) in WORKFLOW_DOCS {
                assert!(
                    linked.contains(*name),
                    "{name} is missing from the Workflow Docs index"
                );
            }
        }
    }

    #[test]
    fn test_render_agents_md() {
        let config = Config {
            version: "1.0.0".to_string(),
            project: crate::config::ProjectConfig {
                name: "test-project".to_string(),
                project_type: vec!["cli".to_string()],
                default_agent: Some("test-dev".to_string()),
                channel: Some("test".to_string()),
                install_command: Some("just install".to_string()),
                release_instructions: None,
                check_command: Some("true".to_string()),
                languages: vec![],
                critical_approvers: None,
            },
            tools: ToolsConfig {
                bones: true,
                maw: true,
                seal: true,
                rite: true,
                vessel: true,
            },
            review: ReviewConfig {
                enabled: true,
                reviewers: vec!["security".to_string()],
            },
            push_main: false,
            agents: crate::config::AgentsConfig::default(),
            models: crate::config::ModelsConfig::default(),
            env: std::collections::HashMap::default(),
        };

        let result = render_agents_md(&config, Layout::Bare).unwrap();

        assert!(result.contains("# test-project"));
        assert!(result.contains("Tools: `bones`, `maw`, `seal`, `rite`, `vessel`"));
        assert!(result.contains("Reviewer roles: security"));
        assert!(result.contains(MANAGED_START));
        assert!(result.contains(MANAGED_END));
        assert!(result.contains("## Edict Workflow"));
    }

    #[test]
    fn test_update_managed_section() {
        let original = r"# My Project

Some custom content.

<!-- edict:managed-start -->
Old managed content here
<!-- edict:managed-end -->

More custom content.
";

        let config = Config {
            version: "1.0.0".to_string(),
            project: crate::config::ProjectConfig {
                name: "test".to_string(),
                project_type: vec!["cli".to_string()],
                default_agent: None,
                channel: None,
                install_command: None,
                release_instructions: None,
                check_command: None,
                languages: vec![],
                critical_approvers: None,
            },
            tools: ToolsConfig {
                bones: true,
                maw: false,
                seal: false,
                rite: false,
                vessel: false,
            },
            review: ReviewConfig {
                enabled: false,
                reviewers: vec![],
            },
            push_main: false,
            agents: crate::config::AgentsConfig::default(),
            models: crate::config::ModelsConfig::default(),
            env: std::collections::HashMap::default(),
        };

        let ctx = TemplateContext::from_config(&config, Layout::Bare);
        let result = update_managed_section(original, &ctx).unwrap();

        assert!(result.contains("# My Project"));
        assert!(result.contains("Some custom content."));
        assert!(result.contains("More custom content."));
        assert!(result.contains(MANAGED_START));
        assert!(result.contains(MANAGED_END));
        assert!(!result.contains("Old managed content"));
        assert!(result.contains("## Edict Workflow"));
    }

    #[test]
    fn test_update_managed_section_migrates_legacy_markers() {
        // AGENTS.md still has botbox:managed-* markers — should be replaced with edict:managed-*
        let original = r"# My Project

Custom content.

<!-- botbox:managed-start -->
Old botbox-era managed content
<!-- botbox:managed-end -->
";

        let config = Config {
            version: "1.0.0".to_string(),
            project: crate::config::ProjectConfig {
                name: "test".to_string(),
                project_type: vec!["cli".to_string()],
                default_agent: None,
                channel: None,
                install_command: None,
                release_instructions: None,
                check_command: None,
                languages: vec![],
                critical_approvers: None,
            },
            tools: ToolsConfig {
                bones: true,
                maw: false,
                seal: false,
                rite: false,
                vessel: false,
            },
            review: ReviewConfig {
                enabled: false,
                reviewers: vec![],
            },
            push_main: false,
            agents: crate::config::AgentsConfig::default(),
            models: crate::config::ModelsConfig::default(),
            env: std::collections::HashMap::default(),
        };

        let ctx = TemplateContext::from_config(&config, Layout::Bare);
        let result = update_managed_section(original, &ctx).unwrap();

        assert!(result.contains("# My Project"));
        assert!(result.contains("Custom content."));
        // Old markers and content gone
        assert!(!result.contains("botbox:managed-start"));
        assert!(!result.contains("botbox:managed-end"));
        assert!(!result.contains("Old botbox-era managed content"));
        // New markers present
        assert!(result.contains(MANAGED_START));
        assert!(result.contains(MANAGED_END));
    }
}
