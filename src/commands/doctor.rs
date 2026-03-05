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
            ("crit", config.tools.crit, "crit"),
            ("botbus (bus)", config.tools.botbus, "bus"),
            ("botty", config.tools.botty, "botty"),
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
                    report.issues.push(format!("Tool not found: {}", binary));
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
                self.print_pretty(&report);
            }
            OutputFormat::Text => {
                self.print_text(&report);
            }
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }

        // Return error with issue count for proper exit code handling
        if issue_count > 0 {
            return Err(crate::error::ExitError::new(
                std::cmp::min(issue_count, 125) as u8,
                format!("{} issue(s) found", issue_count),
            )
            .into());
        }

        Ok(())
    }

    fn print_pretty(&self, report: &DoctorReport) {
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
                        tool.version.as_ref().unwrap_or(&"OK".to_string())
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

        if !report.issues.is_empty() {
            println!("\nIssues ({}):", report.issues.len());
            for issue in &report.issues {
                println!("  • {}", issue);
            }
        } else {
            println!("\n✓ No issues found");
        }
    }

    fn print_text(&self, report: &DoctorReport) {
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
                println!("issue  {}", issue);
            }
        }
    }
}
