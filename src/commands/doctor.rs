use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::subprocess::Tool;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Project root directory
    #[arg(long)]
    pub project_root: Option<PathBuf>,
    /// Strict mode: also verify companion tool versions
    #[arg(long)]
    pub strict: bool,
    /// Output format
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    Pretty,
    Text,
    Json,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub config: ConfigStatus,
    pub tools: Vec<ToolStatus>,
    pub project_files: Vec<FileStatus>,
    pub issues: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigStatus {
    pub project: String,
    pub version: String,
    pub agent: String,
    pub channel: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub enabled: bool,
    pub version: Option<String>,
    pub present: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileStatus {
    pub path: String,
    pub exists: bool,
}

impl DoctorArgs {
    /// Run the doctor checks and print a report.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the project config cannot be located or loaded, if
    /// JSON serialization fails, or if any issues are found (so the process
    /// exits non-zero).
    pub fn execute(&self) -> anyhow::Result<()> {
        let project_root = match self.project_root.clone() {
            Some(p) => p,
            None => std::env::current_dir().context("could not determine current directory")?,
        };

        // Check config at root, then ws/default/ (maw v2 bare repo)
        let (config_path, config_dir) = crate::config::find_config_in_project(&project_root)
            .map_err(|_| anyhow::anyhow!(
                "no .edict.toml or .botbox.toml found at {} or ws/default/ — is this an edict project?",
                project_root.display()
            ))?;
        let project_root = config_dir;
        let config = Config::load(&config_path)?;

        let format = self.format.unwrap_or_else(|| {
            if std::io::stdout().is_terminal() {
                OutputFormat::Pretty
            } else {
                OutputFormat::Text
            }
        });

        let mut report = DoctorReport {
            config: ConfigStatus {
                project: config.project.name.clone(),
                version: config.version.clone(),
                agent: config.default_agent(),
                channel: config.channel(),
            },
            tools: vec![],
            project_files: vec![],
            issues: vec![],
            advice: None,
        };

        Self::check_tools(&mut report, &config);
        Self::check_pi_auth(&mut report, &config);
        Self::check_project_files(&mut report, &project_root);

        // Strict mode: version compatibility check (simplified)
        if self.strict && !report.issues.is_empty() {
            report
                .issues
                .insert(0, "strict mode: found issues".to_string());
        }

        let issue_count = report.issues.len();

        // Format output
        match format {
            OutputFormat::Pretty => {
                Self::print_pretty(&report);
            }
            OutputFormat::Text => {
                Self::print_text(&report);
            }
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }

        // Return error with issue count for proper exit code handling
        if issue_count > 0 {
            return Err(crate::error::ExitError::new(
                u8::try_from(std::cmp::min(issue_count, 125)).unwrap_or(125),
                format!("{issue_count} issue(s) found"),
            )
            .into());
        }

        Ok(())
    }

    /// Probe the agent runtime and required companion tools, recording their
    /// status (and any missing-tool issues) into `report`.
    fn check_tools(report: &mut DoctorReport, config: &Config) {
        // Check tools
        // Always check for Pi (default agent runtime)
        let pi_output = Tool::new("pi").arg("--version").run();
        if let Ok(output) = pi_output {
            report.tools.push(ToolStatus {
                name: "pi (default runtime)".to_string(),
                enabled: true,
                version: Some(output.stdout.trim().to_string()),
                present: true,
            });
        } else {
            report.tools.push(ToolStatus {
                name: "pi (default runtime)".to_string(),
                enabled: true,
                version: None,
                present: false,
            });
            report
                .issues
                .push("Tool not found: pi (default agent runtime)".to_string());
        }

        let required_tools = vec![
            ("bones (bn)", config.tools.bones, "bn"),
            ("maw", config.tools.maw, "maw"),
            ("seal", config.tools.seal, "seal"),
            ("rite", config.tools.rite, "rite"),
            ("vessel", config.tools.vessel, "vessel"),
        ];

        for (label, enabled, binary) in required_tools {
            if enabled {
                let version_output = Tool::new(binary).arg("--version").run();
                if let Ok(output) = version_output {
                    report.tools.push(ToolStatus {
                        name: label.to_string(),
                        enabled: true,
                        version: Some(output.stdout.trim().to_string()),
                        present: true,
                    });
                } else {
                    report.tools.push(ToolStatus {
                        name: label.to_string(),
                        enabled: true,
                        version: None,
                        present: false,
                    });
                    report.issues.push(format!("Tool not found: {binary}"));
                }
            } else {
                report.tools.push(ToolStatus {
                    name: label.to_string(),
                    enabled: false,
                    version: None,
                    present: false,
                });
            }
        }
    }

    /// Verify pi has usable credentials for every provider the project's
    /// agents run through pi.
    ///
    /// An expired pi login fails every agent turn on that provider in under a
    /// second (bn-2d57: the responder's triage model on `openai-codex`), so
    /// surface it here rather than on the channel. Uses `pi auth check`, which
    /// attempts a token refresh, so a revoked refresh token is caught too.
    fn check_pi_auth(report: &mut DoctorReport, config: &Config) {
        if !report
            .tools
            .iter()
            .any(|t| t.name.starts_with("pi ") && t.present)
        {
            return;
        }
        for provider in pi_providers_in_use(config) {
            let output = Tool::new("pi")
                .args(&["auth", "check", "--provider", &provider, "--json"])
                .timeout(std::time::Duration::from_secs(30))
                .run();
            let issue = match output {
                Ok(output) => pi_auth_issue(&provider, &output.stdout),
                Err(e) => Some(format!("pi auth check for {provider} failed to run: {e}")),
            };
            if let Some(issue) = issue {
                report.issues.push(issue);
            }
        }
    }

    /// Check for expected project files/directories, recording their presence
    /// (and any missing-file issues) into `report`.
    fn check_project_files(report: &mut DoctorReport, project_root: &std::path::Path) {
        // Check project files
        let agents_dir = project_root.join(".agents/edict");
        let agents_exists = agents_dir.exists();
        report.project_files.push(FileStatus {
            path: ".agents/edict".to_string(),
            exists: agents_exists,
        });

        if !agents_exists {
            report
                .issues
                .push(".agents/edict/ directory not found".to_string());
        }

        let agents_md = project_root.join("AGENTS.md");
        let agents_md_exists = agents_md.exists();
        report.project_files.push(FileStatus {
            path: "AGENTS.md".to_string(),
            exists: agents_md_exists,
        });

        if !agents_md_exists {
            report.issues.push("AGENTS.md not found".to_string());
        }

        let claude_md = project_root.join("CLAUDE.md");
        let claude_md_exists = claude_md.exists();
        report.project_files.push(FileStatus {
            path: "CLAUDE.md".to_string(),
            exists: claude_md_exists,
        });

        if !claude_md_exists {
            report
                .issues
                .push("CLAUDE.md symlink not found".to_string());
        }
    }

    fn print_pretty(report: &DoctorReport) {
        println!("=== Botbox Doctor ===\n");
        println!("Project: {}", report.config.project);
        println!("Version: {}", report.config.version);
        println!("Agent:   {}", report.config.agent);
        println!("Channel: {}", report.config.channel);
        println!();

        println!("Tools:");
        for tool in &report.tools {
            if tool.enabled {
                if tool.present {
                    println!(
                        "  ✓ {}: {}",
                        tool.name,
                        tool.version.as_deref().unwrap_or("OK")
                    );
                } else {
                    println!("  ✗ {}: NOT FOUND", tool.name);
                }
            } else {
                println!("  - {}: disabled", tool.name);
            }
        }

        if !report.project_files.is_empty() {
            println!("\nProject Files:");
            for file in &report.project_files {
                if file.exists {
                    println!("  ✓ {}", file.path);
                } else {
                    println!("  ✗ {}", file.path);
                }
            }
        }

        if report.issues.is_empty() {
            println!("\n✓ No issues found");
        } else {
            println!("\nIssues ({}):", report.issues.len());
            for issue in &report.issues {
                println!("  • {issue}");
            }
        }
    }

    fn print_text(report: &DoctorReport) {
        println!(
            "edict-doctor  project={}  version={}  agent={}  channel={}",
            report.config.project,
            report.config.version,
            report.config.agent,
            report.config.channel
        );

        for tool in &report.tools {
            let status = if !tool.enabled {
                "disabled".to_string()
            } else if tool.present {
                format!("ok  {}", tool.version.as_ref().unwrap_or(&String::new()))
            } else {
                "missing".to_string()
            };
            println!("tool  {}  {}", tool.name, status);
        }

        for file in &report.project_files {
            let status = if file.exists { "ok" } else { "missing" };
            println!("file  {}  {}", file.path, status);
        }

        if !report.issues.is_empty() {
            println!("issues  count={}", report.issues.len());
            for issue in &report.issues {
                println!("issue  {issue}");
            }
        }
    }
}

/// Providers the project's agents reach through the pi runner (anything that
/// is not `anthropic/...`, which runs on Claude Code), sorted and deduplicated.
fn pi_providers_in_use(config: &Config) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    let responder_model = config
        .agents
        .responder
        .as_ref()
        .map_or("sonnet", |r| r.model.as_str());
    models.extend(config.resolve_model_pool(responder_model));
    if config.tools.rite {
        models.push(crate::commands::responder::TRIAGE_MODEL.to_string());
    }
    if let Some(dev) = &config.agents.dev {
        models.extend(config.resolve_model_pool(&dev.model));
    }
    if let Some(worker) = &config.agents.worker {
        models.extend(config.resolve_model_pool(&worker.model));
    }

    let mut providers: Vec<String> = models
        .iter()
        .filter_map(|m| m.split_once('/').map(|(provider, _)| provider))
        .filter(|provider| *provider != "anthropic")
        .map(str::to_string)
        .collect();
    providers.sort();
    providers.dedup();
    providers
}

/// Turn `pi auth check --json` output into a doctor issue, or `None` when the
/// provider is ready.
fn pi_auth_issue(provider: &str, stdout: &str) -> Option<String> {
    let parsed: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    let status = parsed
        .as_ref()
        .and_then(|v| v.get("status"))
        .and_then(serde_json::Value::as_str);
    if status == Some("ready") {
        return None;
    }
    let reason = parsed
        .as_ref()
        .and_then(|v| v.get("reason"))
        .and_then(serde_json::Value::as_str)
        .or(status)
        .unwrap_or("unrecognised output");
    Some(format!(
        "pi credentials for {provider} are not usable ({reason}): run `pi` and use /login to sign in to {provider}, then check with `pi auth check --provider {provider}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(toml: &str) -> Config {
        Config::parse_toml(toml).unwrap()
    }

    /// bn-2d57: rite's config. Its dev agent and responder run on Claude Code,
    /// but triage runs on openai-codex through pi, so doctor must check it.
    #[test]
    fn rite_config_needs_openai_codex_through_pi() {
        let c = config(
            r#"version = "1.0.15"
[project]
name = "rite"
[tools]
rite = true
[agents.dev]
model = "opus"
"#,
        );
        assert_eq!(pi_providers_in_use(&c), vec!["openai-codex".to_string()]);
    }

    #[test]
    fn anthropic_only_project_without_rite_needs_no_pi_auth() {
        let c = config(
            r#"version = "1.0.15"
[project]
name = "x"
[tools]
rite = false
[agents.dev]
model = "opus"
"#,
        );
        assert!(pi_providers_in_use(&c).is_empty());
    }

    #[test]
    fn revoked_login_is_an_issue() {
        // Exact output of `pi auth check --provider openai-codex --json` with a
        // revoked refresh token.
        let issue = pi_auth_issue(
            "openai-codex",
            r#"{"status":"invalid","provider":"openai-codex","reason":"invalid_state"}"#,
        )
        .unwrap();
        assert!(issue.contains("openai-codex"));
        assert!(issue.contains("invalid_state"));
        assert!(issue.contains("/login"));
    }

    #[test]
    fn ready_provider_is_not_an_issue() {
        assert!(
            pi_auth_issue(
                "openai-codex",
                r#"{"status":"ready","provider":"openai-codex","authType":"oauth"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn unparseable_output_is_an_issue() {
        assert!(pi_auth_issue("openai-codex", "").is_some());
    }
}
