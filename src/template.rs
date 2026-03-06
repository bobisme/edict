//! Template rendering for docs, prompts, and AGENTS.md managed section.

use minijinja::Environment;
use serde::Serialize;

use crate::config::{Config, ReviewConfig, ToolsConfig};

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
    /// Install command (optional)
    pub install_command: Option<String>,
    /// Check command run before merging (optional)
    pub check_command: Option<String>,
    /// Workflow docs with descriptions
    pub workflow_docs: Vec<DocEntry>,
    /// Design docs with descriptions (filtered by project type)
    pub design_docs: Vec<DocEntry>,
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
    /// Build template context from project config
    pub fn from_config(config: &Config) -> Self {
        let workflow_docs = list_workflow_docs();
        let design_docs = list_design_docs(&config.project.project_type);

        Self {
            project: ProjectInfo {
                name: config.project.name.clone(),
                project_type: config.project.project_type.clone(),
                default_agent: config.project.default_agent.clone(),
                channel: config.project.channel.clone(),
            },
            tools: config.tools.clone(),
            review: config.review.clone(),
            install_command: config.project.install_command.clone(),
            check_command: config.project.check_command.clone(),
            workflow_docs,
            design_docs,
        }
    }
}

/// List all workflow docs with descriptions
fn list_workflow_docs() -> Vec<DocEntry> {
    vec![
        DocEntry {
            name: "triage.md".to_string(),
            description: "Find work from inbox and bones".to_string(),
        },
        DocEntry {
            name: "start.md".to_string(),
            description: "Claim bone, create workspace, announce".to_string(),
        },
        DocEntry {
            name: "update.md".to_string(),
            description: "Change bone state (open/doing/done)".to_string(),
        },
        DocEntry {
            name: "finish.md".to_string(),
            description: "Close bone, merge workspace, release claims".to_string(),
        },
        DocEntry {
            name: "worker-loop.md".to_string(),
            description: "Full triage-work-finish lifecycle".to_string(),
        },
        DocEntry {
            name: "planning.md".to_string(),
            description: "Turn specs/PRDs into actionable bones".to_string(),
        },
        DocEntry {
            name: "scout.md".to_string(),
            description: "Explore unfamiliar code before planning".to_string(),
        },
        DocEntry {
            name: "proposal.md".to_string(),
            description: "Create and validate proposals before implementation".to_string(),
        },
        DocEntry {
            name: "review-request.md".to_string(),
            description: "Request a review".to_string(),
        },
        DocEntry {
            name: "review-response.md".to_string(),
            description: "Handle reviewer feedback (fix/address/defer)".to_string(),
        },
        DocEntry {
            name: "review-loop.md".to_string(),
            description: "Reviewer agent loop".to_string(),
        },
        DocEntry {
            name: "merge-check.md".to_string(),
            description: "Merge a worker workspace (protocol merge + conflict recovery)"
                .to_string(),
        },
        DocEntry {
            name: "preflight.md".to_string(),
            description: "Validate toolchain health".to_string(),
        },
        DocEntry {
            name: "cross-channel.md".to_string(),
            description: "Ask questions, report bugs, and track responses across projects"
                .to_string(),
        },
        DocEntry {
            name: "report-issue.md".to_string(),
            description: "Report bugs/features to other projects".to_string(),
        },
        DocEntry {
            name: "groom.md".to_string(),
            description: "groom".to_string(),
        },
    ]
}

/// List design docs filtered by project types
fn list_design_docs(project_types: &[String]) -> Vec<DocEntry> {
    let mut docs = Vec::new();

    // cli-conventions is eligible for all project types
    for _project_type in project_types {
        docs.push(DocEntry {
            name: "cli-conventions.md".to_string(),
            description: "CLI tool design for humans, agents, and machines".to_string(),
        });
        break; // Add once, not per type
    }

    docs
}

/// Render the AGENTS.md managed section
pub fn render_managed_section(ctx: &TemplateContext) -> anyhow::Result<String> {
    let mut env = Environment::new();
    env.add_template("agents-managed", AGENTS_MANAGED_TEMPLATE)?;

    let template = env.get_template("agents-managed")?;
    let rendered = template.render(ctx)?;

    Ok(rendered)
}

/// Render a complete AGENTS.md file for a new project
pub fn render_agents_md(config: &Config) -> anyhow::Result<String> {
    let ctx = TemplateContext::from_config(config);

    let tool_list = config
        .tools
        .enabled_tools()
        .into_iter()
        .map(|t| format!("`{}`", t))
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
pub fn update_managed_section(content: &str, ctx: &TemplateContext) -> anyhow::Result<String> {
    let managed = render_managed_section(ctx)?;
    let full_managed = format!("{}\n{}\n{}", MANAGED_START, managed, MANAGED_END);

    // Try current markers first
    if let Some(start_idx) = content.find(MANAGED_START)
        && let Some(end_idx) = content.find(MANAGED_END)
        && end_idx > start_idx
    {
        let before = &content[..start_idx];
        let after = &content[end_idx + MANAGED_END.len()..];
        return Ok(format!("{}{}{}", before, full_managed, after));
    }

    // Try legacy markers (botbox era) — replace them with current markers
    if let Some(start_idx) = content.find(MANAGED_START_LEGACY)
        && let Some(end_idx) = content.find(MANAGED_END_LEGACY)
        && end_idx > start_idx
    {
        let before = &content[..start_idx];
        let after = &content[end_idx + MANAGED_END_LEGACY.len()..];
        return Ok(format!("{}{}{}", before, full_managed, after));
    }

    // Missing or invalid markers — strip any stale marker fragments and append
    let temp = content
        .replace(MANAGED_START, "")
        .replace(MANAGED_END, "")
        .replace(MANAGED_START_LEGACY, "")
        .replace(MANAGED_END_LEGACY, "");
    let cleaned = temp.trim_end();
    Ok(format!("{}\n\n{}\n", cleaned, full_managed))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            agents: Default::default(),
            models: Default::default(),
            env: Default::default(),
        };

        let result = render_agents_md(&config).unwrap();

        assert!(result.contains("# test-project"));
        assert!(result.contains("Tools: `bones`, `maw`, `seal`, `rite`, `vessel`"));
        assert!(result.contains("Reviewer roles: security"));
        assert!(result.contains(MANAGED_START));
        assert!(result.contains(MANAGED_END));
        assert!(result.contains("## Edict Workflow"));
    }

    #[test]
    fn test_update_managed_section() {
        let original = r#"# My Project

Some custom content.

<!-- edict:managed-start -->
Old managed content here
<!-- edict:managed-end -->

More custom content.
"#;

        let config = Config {
            version: "1.0.0".to_string(),
            project: crate::config::ProjectConfig {
                name: "test".to_string(),
                project_type: vec!["cli".to_string()],
                default_agent: None,
                channel: None,
                install_command: None,
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
            agents: Default::default(),
            models: Default::default(),
            env: Default::default(),
        };

        let ctx = TemplateContext::from_config(&config);
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
        let original = r#"# My Project

Custom content.

<!-- botbox:managed-start -->
Old botbox-era managed content
<!-- botbox:managed-end -->
"#;

        let config = Config {
            version: "1.0.0".to_string(),
            project: crate::config::ProjectConfig {
                name: "test".to_string(),
                project_type: vec!["cli".to_string()],
                default_agent: None,
                channel: None,
                install_command: None,
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
            agents: Default::default(),
            models: Default::default(),
            env: Default::default(),
        };

        let ctx = TemplateContext::from_config(&config);
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
