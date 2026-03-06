use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rand::seq::{IndexedRandom, SliceRandom};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ExitError;

/// Config file name constants.
pub const CONFIG_TOML: &str = ".edict.toml";
/// Legacy config name from the botbox era — accepted on read, migrated to CONFIG_TOML on sync.
pub const CONFIG_TOML_LEGACY: &str = ".botbox.toml";
pub const CONFIG_JSON: &str = ".botbox.json";

/// Find the config file path, preferring the current name over legacy names.
/// Returns None if none exist.
pub fn find_config(dir: &Path) -> Option<PathBuf> {
    // Current name
    let toml_path = dir.join(CONFIG_TOML);
    if toml_path.exists() {
        return Some(toml_path);
    }
    // Legacy TOML name (botbox era, migrated to .edict.toml on sync)
    let legacy_toml_path = dir.join(CONFIG_TOML_LEGACY);
    if legacy_toml_path.exists() {
        return Some(legacy_toml_path);
    }
    // Oldest legacy JSON name
    let json_path = dir.join(CONFIG_JSON);
    if json_path.exists() {
        return Some(json_path);
    }
    None
}

/// Find config in the standard locations: direct path, then ws/default/.
/// Returns (config_path, config_dir) or an error.
///
/// Priority order (highest first):
/// 1. Root `.edict.toml` — current canonical name
/// 2. `ws/default/.edict.toml` — maw v2 bare repo, current name
/// 3. Root `.botbox.toml` — legacy name, migrated to .edict.toml on sync
/// 4. `ws/default/.botbox.toml` — maw v2 bare repo, legacy name
/// 5. Root `.botbox.json` — oldest legacy format
/// 6. `ws/default/.botbox.json` — maw v2 oldest legacy
///
/// In maw v2 bare repos, the TOML migration runs inside ws/default. This can leave a
/// stale `.botbox.json` at the bare root (with wrong project name / agent identity) that
/// would previously shadow the correct ws/default TOML. By checking ws/default TOML before
/// root JSON we ensure agents always load the current, migrated config.
pub fn find_config_in_project(root: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let ws_default = root.join("ws/default");

    // 1. Root .edict.toml — current canonical name
    let root_toml = root.join(CONFIG_TOML);
    if root_toml.exists() {
        return Ok((root_toml, root.to_path_buf()));
    }

    // 2. ws/default .edict.toml — maw v2 bare repo, current name
    let ws_toml = ws_default.join(CONFIG_TOML);
    if ws_toml.exists() {
        return Ok((ws_toml, ws_default));
    }

    // 3. Root .botbox.toml — legacy name, will be migrated on sync
    let root_legacy_toml = root.join(CONFIG_TOML_LEGACY);
    if root_legacy_toml.exists() {
        return Ok((root_legacy_toml, root.to_path_buf()));
    }

    // 4. ws/default .botbox.toml — maw v2 legacy
    let ws_legacy_toml = ws_default.join(CONFIG_TOML_LEGACY);
    if ws_legacy_toml.exists() {
        return Ok((ws_legacy_toml, ws_default));
    }

    // 5. Root JSON — oldest legacy format
    let root_json = root.join(CONFIG_JSON);
    if root_json.exists() {
        return Ok((root_json, root.to_path_buf()));
    }

    // 6. ws/default JSON — maw v2 not yet migrated
    let ws_json = ws_default.join(CONFIG_JSON);
    if ws_json.exists() {
        return Ok((ws_json, ws_default));
    }

    anyhow::bail!(
        "no .edict.toml or .botbox.toml found in {} or ws/default/",
        root.display()
    )
}

/// Top-level .botbox.toml config.
///
/// All structs use snake_case (TOML native) with `alias` attributes for
/// backwards compatibility when loading legacy camelCase JSON configs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    pub version: String,
    pub project: ProjectConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default, alias = "pushMain")]
    pub push_main: bool,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub models: ModelsConfig,
    /// Environment variables to pass to all spawned agents.
    /// Values support shell variable expansion (e.g. `$HOME`, `${HOME}`).
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectConfig {
    pub name: String,
    #[serde(default, rename = "type")]
    pub project_type: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default, alias = "defaultAgent")]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default, alias = "installCommand")]
    pub install_command: Option<String>,
    #[serde(default, alias = "checkCommand")]
    pub check_command: Option<String>,
    #[serde(default, alias = "criticalApprovers")]
    pub critical_approvers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ToolsConfig {
    #[serde(default, alias = "beads")]
    pub bones: bool,
    #[serde(default)]
    pub maw: bool,
    #[serde(default, alias = "crit")]
    pub seal: bool,
    #[serde(default)]
    pub botbus: bool,
    #[serde(default, alias = "botty")]
    pub vessel: bool,
}

impl ToolsConfig {
    /// Returns a list of enabled tool names
    pub fn enabled_tools(&self) -> Vec<String> {
        let mut tools = Vec::new();
        if self.bones {
            tools.push("bones".to_string());
        }
        if self.maw {
            tools.push("maw".to_string());
        }
        if self.seal {
            tools.push("seal".to_string());
        }
        if self.botbus {
            tools.push("botbus".to_string());
        }
        if self.vessel {
            tools.push("vessel".to_string());
        }
        tools
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ReviewConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub reviewers: Vec<String>,
}

/// Model tier configuration for cross-provider load balancing.
///
/// Each tier maps to a list of `provider/model:thinking` strings.
/// When an agent config specifies a tier name (e.g. "fast"), `resolve_model()`
/// randomly picks one model from that tier's pool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelsConfig {
    #[serde(default = "default_tier_fast")]
    pub fast: Vec<String>,
    #[serde(default = "default_tier_balanced")]
    pub balanced: Vec<String>,
    #[serde(default = "default_tier_strong")]
    pub strong: Vec<String>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            fast: default_tier_fast(),
            balanced: default_tier_balanced(),
            strong: default_tier_strong(),
        }
    }
}

fn default_tier_fast() -> Vec<String> {
    vec![
        "anthropic/claude-haiku-4-5:low".into(),
        "google-gemini-cli/gemini-3-flash-preview:low".into(),
        "openai-codex/gpt-5.3-codex-spark:low".into(),
    ]
}

fn default_tier_balanced() -> Vec<String> {
    vec![
        "anthropic/claude-sonnet-4-6:medium".into(),
        "google-gemini-cli/gemini-3-pro-preview:medium".into(),
        "openai-codex/gpt-5.3-codex:medium".into(),
    ]
}

fn default_tier_strong() -> Vec<String> {
    vec![
        "anthropic/claude-opus-4-6:high".into(),
        "openai-codex/gpt-5.3-codex:xhigh".into(),
    ]
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentsConfig {
    #[serde(default)]
    pub dev: Option<DevAgentConfig>,
    #[serde(default)]
    pub worker: Option<WorkerAgentConfig>,
    #[serde(default)]
    pub reviewer: Option<ReviewerAgentConfig>,
    #[serde(default)]
    pub responder: Option<ResponderAgentConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DevAgentConfig {
    #[serde(default = "default_model_dev")]
    pub model: String,
    #[serde(default = "default_max_loops", alias = "maxLoops")]
    pub max_loops: u32,
    #[serde(default = "default_pause")]
    pub pause: u32,
    #[serde(default = "default_timeout_3600")]
    pub timeout: u64,
    #[serde(default = "default_missions")]
    pub missions: Option<MissionsConfig>,
    #[serde(default = "default_multi_lead", alias = "multiLead")]
    pub multi_lead: Option<MultiLeadConfig>,
    /// Memory limit for dev-loop agents (e.g. "4G", "2G"). Passed as --memory-limit to vessel spawn.
    #[serde(default)]
    pub memory_limit: Option<String>,
}

impl Default for DevAgentConfig {
    fn default() -> Self {
        Self {
            model: default_model_dev(),
            max_loops: default_max_loops(),
            pause: default_pause(),
            timeout: default_timeout_3600(),
            missions: default_missions(),
            multi_lead: default_multi_lead(),
            memory_limit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MissionsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_workers", alias = "maxWorkers")]
    pub max_workers: u32,
    #[serde(default = "default_max_children", alias = "maxChildren")]
    pub max_children: u32,
    #[serde(
        default = "default_checkpoint_interval",
        alias = "checkpointIntervalSec"
    )]
    pub checkpoint_interval_sec: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MultiLeadConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_leads", alias = "maxLeads")]
    pub max_leads: u32,
    #[serde(default = "default_merge_timeout", alias = "mergeTimeoutSec")]
    pub merge_timeout_sec: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkerAgentConfig {
    #[serde(default = "default_model_worker")]
    pub model: String,
    #[serde(default = "default_timeout_900")]
    pub timeout: u64,
    /// Memory limit for worker agents (e.g. "4G", "2G"). Passed as --memory-limit to vessel spawn.
    #[serde(default)]
    pub memory_limit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewerAgentConfig {
    #[serde(default = "default_model_reviewer")]
    pub model: String,
    #[serde(default = "default_max_loops", alias = "maxLoops")]
    pub max_loops: u32,
    #[serde(default = "default_pause")]
    pub pause: u32,
    #[serde(default = "default_timeout_900")]
    pub timeout: u64,
    /// Memory limit for reviewer agents (e.g. "4G", "2G"). Passed as --memory-limit to vessel spawn.
    #[serde(default)]
    pub memory_limit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResponderAgentConfig {
    #[serde(default = "default_model_responder")]
    pub model: String,
    #[serde(default = "default_timeout_300")]
    pub timeout: u64,
    #[serde(default = "default_timeout_300")]
    pub wait_timeout: u64,
    #[serde(default = "default_max_conversations", alias = "maxConversations")]
    pub max_conversations: u32,
    /// Memory limit for responder agents (e.g. "4G", "2G"). Passed as --memory-limit to vessel spawn.
    #[serde(default)]
    pub memory_limit: Option<String>,
}

// Default value functions for serde
fn default_model_dev() -> String {
    "opus".into()
}
fn default_model_worker() -> String {
    "balanced".into()
}
fn default_model_reviewer() -> String {
    "strong".into()
}
fn default_model_responder() -> String {
    "balanced".into()
}
fn default_max_loops() -> u32 {
    100
}
fn default_pause() -> u32 {
    2
}
fn default_timeout_300() -> u64 {
    300
}
fn default_timeout_900() -> u64 {
    900
}
fn default_timeout_3600() -> u64 {
    3600
}
fn default_true() -> bool {
    true
}
fn default_max_workers() -> u32 {
    4
}
fn default_max_children() -> u32 {
    12
}
fn default_checkpoint_interval() -> u64 {
    30
}
fn default_max_leads() -> u32 {
    3
}
fn default_merge_timeout() -> u64 {
    120
}
fn default_max_conversations() -> u32 {
    10
}
fn default_missions() -> Option<MissionsConfig> {
    Some(MissionsConfig::default())
}
fn default_multi_lead() -> Option<MultiLeadConfig> {
    Some(MultiLeadConfig::default())
}

impl Default for MissionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_workers: default_max_workers(),
            max_children: default_max_children(),
            checkpoint_interval_sec: default_checkpoint_interval(),
        }
    }
}

impl Default for MultiLeadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_leads: default_max_leads(),
            merge_timeout_sec: default_merge_timeout(),
        }
    }
}

impl Config {
    /// Load config from a file (TOML or JSON, auto-detected by extension).
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "toml" => Self::parse_toml(&contents),
            "json" => Self::parse_json(&contents),
            _ => {
                // Try TOML first, then JSON
                Self::parse_toml(&contents).or_else(|_| Self::parse_json(&contents))
            }
        }
    }

    /// Parse config from a TOML string.
    pub fn parse_toml(toml_str: &str) -> anyhow::Result<Self> {
        toml::from_str(toml_str)
            .map_err(|e| ExitError::Config(format!("invalid .edict.toml: {e}")).into())
    }

    /// Parse config from a JSON string (for backwards compatibility).
    pub fn parse_json(json: &str) -> anyhow::Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| ExitError::Config(format!("invalid .botbox.json: {e}")).into())
    }

    /// Serialize config to a TOML string with helpful comments.
    pub fn to_toml(&self) -> anyhow::Result<String> {
        let raw = toml::to_string_pretty(self).context("serializing config to TOML")?;

        // Use toml_edit to add comments for default values
        let mut doc: toml_edit::DocumentMut = raw
            .parse()
            .context("parsing generated TOML for comment injection")?;

        // Add header comment with taplo schema reference for editor autocomplete
        doc.decor_mut().set_prefix(
            "#:schema https://raw.githubusercontent.com/bobisme/edict/main/schemas/edict.schema.json\n\
             # Edict project configuration\n\
             # Schema: https://github.com/bobisme/edict/blob/main/schemas/edict.schema.json\n\n",
        );

        // Add comments before section headers using item decor
        fn set_table_comment(doc: &mut toml_edit::DocumentMut, key: &str, comment: &str) {
            if let Some(item) = doc.get_mut(key) {
                if let Some(tbl) = item.as_table_mut() {
                    tbl.decor_mut().set_prefix(comment);
                }
            }
        }

        set_table_comment(&mut doc, "tools", "\n# Companion tools to enable\n");
        set_table_comment(&mut doc, "review", "\n# Code review configuration\n");
        set_table_comment(
            &mut doc,
            "agents",
            "\n# Agent configuration (omit sections to use defaults)\n",
        );
        set_table_comment(
            &mut doc,
            "models",
            "\n# Model tier pools for load balancing\n# Each tier maps to a list of \"provider/model:thinking\" strings\n",
        );
        set_table_comment(
            &mut doc,
            "env",
            "\n# Environment variables passed to all spawned agents\n# Values support shell variable expansion ($HOME, ${HOME})\n# Set OTEL_EXPORTER_OTLP_ENDPOINT to enable telemetry: \"stderr\" for JSON to stderr, \"http://host:port\" for OTLP HTTP\n",
        );

        Ok(doc.to_string())
    }

    /// Returns the effective agent name (project.default_agent or "{name}-dev").
    pub fn default_agent(&self) -> String {
        self.project
            .default_agent
            .clone()
            .unwrap_or_else(|| format!("{}-dev", self.project.name))
    }

    /// Returns the effective channel name (project.channel or project.name).
    pub fn channel(&self) -> String {
        self.project
            .channel
            .clone()
            .unwrap_or_else(|| self.project.name.clone())
    }

    /// Returns env vars with shell variables expanded (e.g. `$HOME` → `/home/user`).
    ///
    /// Also propagates `OTEL_EXPORTER_OTLP_ENDPOINT` from the process environment if set and
    /// not already defined in the config, so telemetry flows through to spawned agents.
    pub fn resolved_env(&self) -> HashMap<String, String> {
        let mut env: HashMap<String, String> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand_env_value(v)))
            .collect();

        // Auto-propagate telemetry endpoint to child agents
        if !env.contains_key("OTEL_EXPORTER_OTLP_ENDPOINT") {
            if let Ok(val) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
                env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), val);
            }
        }

        env
    }

    /// Resolve a model string to the full pool of models for that tier.
    /// Tier names (fast/balanced/strong) return a shuffled Vec of all models in the pool.
    /// Legacy short names (opus/sonnet/haiku) and explicit model strings return a single-element Vec.
    pub fn resolve_model_pool(&self, model: &str) -> Vec<String> {
        // Legacy short names -> specific Anthropic models (no fallback pool)
        match model {
            "opus" => return vec!["anthropic/claude-opus-4-6:high".to_string()],
            "sonnet" => return vec!["anthropic/claude-sonnet-4-6:medium".to_string()],
            "haiku" => return vec!["anthropic/claude-haiku-4-5:low".to_string()],
            _ => {}
        }

        // Tier names -> shuffled pool
        let pool = match model {
            "fast" => &self.models.fast,
            "balanced" => &self.models.balanced,
            "strong" => &self.models.strong,
            _ => return vec![model.to_string()],
        };

        if pool.is_empty() {
            return vec![model.to_string()];
        }

        let mut pool = pool.clone();
        pool.shuffle(&mut rand::rng());
        pool
    }

    /// Resolve a model string: if it matches a tier name (fast/balanced/strong),
    /// randomly pick from that tier's pool. Otherwise pass through as-is.
    pub fn resolve_model(&self, model: &str) -> String {
        // Legacy short names -> specific Anthropic models (deterministic)
        match model {
            "opus" => return "anthropic/claude-opus-4-6:high".to_string(),
            "sonnet" => return "anthropic/claude-sonnet-4-6:medium".to_string(),
            "haiku" => return "anthropic/claude-haiku-4-5:low".to_string(),
            _ => {}
        }

        // Tier names -> random pool selection
        let pool = match model {
            "fast" => &self.models.fast,
            "balanced" => &self.models.balanced,
            "strong" => &self.models.strong,
            _ => return model.to_string(),
        };

        if pool.is_empty() {
            return model.to_string();
        }

        let mut rng = rand::rng();
        pool.choose(&mut rng)
            .cloned()
            .unwrap_or_else(|| model.to_string())
    }
}

/// Expand shell-style variable references in a string.
/// Supports `$VAR` and `${VAR}` syntax. Unknown variables are left as-is.
fn expand_env_value(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            // ${VAR} syntax
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
                if let Ok(val) = std::env::var(&var_name) {
                    result.push_str(&val);
                } else {
                    result.push_str(&format!("${{{var_name}}}"));
                }
            } else {
                // $VAR syntax — peek-collect alphanumeric + underscore
                let mut var_name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        var_name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if var_name.is_empty() {
                    result.push('$');
                } else if let Ok(val) = std::env::var(&var_name) {
                    result.push_str(&val);
                } else {
                    result.push('$');
                    result.push_str(&var_name);
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Convert a JSON config string to TOML format.
/// Used during migration from .botbox.json to .botbox.toml.
pub fn json_to_toml(json: &str) -> anyhow::Result<String> {
    let config = Config::parse_json(json)?;
    config.to_toml()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_toml_config() {
        let toml_str = r#"
version = "1.0.16"
push_main = false

[project]
name = "myapp"
type = ["cli"]
channel = "myapp"
install_command = "just install"
check_command = "cargo clippy && cargo test"
default_agent = "myapp-dev"

[tools]
bones = true
maw = true
seal = true
botbus = true
vessel = true

[review]
enabled = true
reviewers = ["security"]

[agents.dev]
model = "opus"
max_loops = 20
pause = 2
timeout = 900

[agents.worker]
model = "haiku"
timeout = 600

[agents.reviewer]
model = "opus"
max_loops = 20
pause = 2
timeout = 600
"#;

        let config = Config::parse_toml(toml_str).unwrap();
        assert_eq!(config.project.name, "myapp");
        assert_eq!(config.default_agent(), "myapp-dev");
        assert_eq!(config.channel(), "myapp");
        assert!(config.tools.bones);
        assert!(config.tools.maw);
        assert!(config.review.enabled);
        assert_eq!(config.review.reviewers, vec!["security"]);
        assert!(!config.push_main);
        assert_eq!(
            config.project.check_command,
            Some("cargo clippy && cargo test".to_string())
        );

        let dev = config.agents.dev.unwrap();
        assert_eq!(dev.model, "opus");
        assert_eq!(dev.max_loops, 20);
        assert_eq!(dev.timeout, 900);

        let worker = config.agents.worker.unwrap();
        assert_eq!(worker.model, "haiku");
        assert_eq!(worker.timeout, 600);
    }

    #[test]
    fn parse_full_json_config_with_camel_case() {
        let json = r#"{
            "version": "1.0.16",
            "project": {
                "name": "myapp",
                "type": ["cli"],
                "channel": "myapp",
                "installCommand": "just install",
                "checkCommand": "cargo clippy && cargo test",
                "defaultAgent": "myapp-dev"
            },
            "tools": { "bones": true, "maw": true, "seal": true, "botbus": true, "vessel": true },
            "review": { "enabled": true, "reviewers": ["security"] },
            "pushMain": false,
            "agents": {
                "dev": { "model": "opus", "maxLoops": 20, "pause": 2, "timeout": 900 },
                "worker": { "model": "haiku", "timeout": 600 },
                "reviewer": { "model": "opus", "maxLoops": 20, "pause": 2, "timeout": 600 }
            }
        }"#;

        let config = Config::parse_json(json).unwrap();
        assert_eq!(config.project.name, "myapp");
        assert_eq!(config.default_agent(), "myapp-dev");
        assert_eq!(config.channel(), "myapp");
        assert!(config.tools.bones);
        assert!(config.review.enabled);
        assert!(!config.push_main);

        let dev = config.agents.dev.unwrap();
        assert_eq!(dev.model, "opus");
        assert_eq!(dev.max_loops, 20);
    }

    #[test]
    fn parse_minimal_toml_config() {
        let toml_str = r#"
version = "1.0.0"

[project]
name = "test"
"#;

        let config = Config::parse_toml(toml_str).unwrap();
        assert_eq!(config.project.name, "test");
        assert_eq!(config.default_agent(), "test-dev");
        assert_eq!(config.channel(), "test");
        assert!(!config.tools.bones);
        assert!(!config.review.enabled);
        assert!(!config.push_main);
        assert!(config.agents.dev.is_none());
    }

    #[test]
    fn parse_missing_optional_fields() {
        let toml_str = r#"
version = "1.0.0"

[project]
name = "bare"

[agents.dev]
model = "sonnet"
"#;

        let config = Config::parse_toml(toml_str).unwrap();
        let dev = config.agents.dev.unwrap();
        assert_eq!(dev.model, "sonnet");
        assert_eq!(dev.max_loops, 100); // default
        assert_eq!(dev.pause, 2); // default
        assert_eq!(dev.timeout, 3600); // default
    }

    #[test]
    fn resolve_model_tier_names() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        let fast = config.resolve_model("fast");
        assert!(
            fast.contains('/'),
            "fast tier should resolve to provider/model, got: {fast}"
        );

        let balanced = config.resolve_model("balanced");
        assert!(
            balanced.contains('/'),
            "balanced tier should resolve to provider/model, got: {balanced}"
        );

        let strong = config.resolve_model("strong");
        assert!(
            strong.contains('/'),
            "strong tier should resolve to provider/model, got: {strong}"
        );
    }

    #[test]
    fn resolve_model_passthrough() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        assert_eq!(
            config.resolve_model("anthropic/claude-sonnet-4-6:medium"),
            "anthropic/claude-sonnet-4-6:medium"
        );
        assert_eq!(
            config.resolve_model("some-unknown-model"),
            "some-unknown-model"
        );
        assert_eq!(
            config.resolve_model("opus"),
            "anthropic/claude-opus-4-6:high"
        );
        assert_eq!(
            config.resolve_model("sonnet"),
            "anthropic/claude-sonnet-4-6:medium"
        );
        assert_eq!(
            config.resolve_model("haiku"),
            "anthropic/claude-haiku-4-5:low"
        );
    }

    #[test]
    fn resolve_model_custom_tiers() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
[models]
fast = ["custom/model-a"]
balanced = ["custom/model-b"]
strong = ["custom/model-c"]
"#,
        )
        .unwrap();

        assert_eq!(config.resolve_model("fast"), "custom/model-a");
        assert_eq!(config.resolve_model("balanced"), "custom/model-b");
        assert_eq!(config.resolve_model("strong"), "custom/model-c");
    }

    #[test]
    fn default_model_tiers() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        assert!(!config.models.fast.is_empty());
        assert!(!config.models.balanced.is_empty());
        assert!(!config.models.strong.is_empty());
    }

    #[test]
    fn resolve_model_pool_tiers() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        let pool = config.resolve_model_pool("balanced");
        assert_eq!(pool.len(), 3, "balanced tier should have 3 models");
        assert!(
            pool.iter().all(|m| m.contains('/')),
            "all models should be provider/model format"
        );
    }

    #[test]
    fn resolve_model_pool_legacy_names() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        assert_eq!(
            config.resolve_model_pool("opus"),
            vec!["anthropic/claude-opus-4-6:high"]
        );
        assert_eq!(
            config.resolve_model_pool("sonnet"),
            vec!["anthropic/claude-sonnet-4-6:medium"]
        );
        assert_eq!(
            config.resolve_model_pool("haiku"),
            vec!["anthropic/claude-haiku-4-5:low"]
        );
    }

    #[test]
    fn resolve_model_pool_explicit_model() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
"#,
        )
        .unwrap();

        assert_eq!(
            config.resolve_model_pool("anthropic/claude-sonnet-4-6:medium"),
            vec!["anthropic/claude-sonnet-4-6:medium"]
        );
    }

    #[test]
    fn parse_malformed_toml() {
        let result = Config::parse_toml("not valid toml [[[");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid .edict.toml"));
    }

    #[test]
    fn parse_malformed_json() {
        let result = Config::parse_json("not json");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid .botbox.json"));
    }

    #[test]
    fn parse_missing_required_fields() {
        let toml_str = r#"version = "1.0.0""#;
        let result = Config::parse_toml(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn roundtrip_toml() {
        let toml_str = r#"
version = "1.0.16"

[project]
name = "myapp"
type = ["cli"]
default_agent = "myapp-dev"
channel = "myapp"
install_command = "just install"

[tools]
bones = true
maw = true
seal = true
botbus = true
vessel = true
"#;

        let config = Config::parse_toml(toml_str).unwrap();
        let output = config.to_toml().unwrap();
        let config2 = Config::parse_toml(&output).unwrap();
        assert_eq!(config.project.name, config2.project.name);
        assert_eq!(config.project.default_agent, config2.project.default_agent);
        assert_eq!(config.tools.bones, config2.tools.bones);
    }

    #[test]
    fn json_to_toml_conversion() {
        let json = r#"{
            "version": "1.0.16",
            "project": {
                "name": "test",
                "type": ["cli"],
                "defaultAgent": "test-dev",
                "channel": "test"
            },
            "tools": { "bones": true, "maw": true },
            "pushMain": false
        }"#;

        let toml_str = json_to_toml(json).unwrap();
        let config = Config::parse_toml(&toml_str).unwrap();
        assert_eq!(config.project.name, "test");
        assert_eq!(config.project.default_agent, Some("test-dev".to_string()));
        assert!(config.tools.bones);
        assert!(config.tools.maw);
        assert!(!config.push_main);
    }

    #[test]
    fn find_config_prefers_edict_toml() {
        let dir = tempfile::tempdir().unwrap();
        // All three exist — .edict.toml wins
        std::fs::write(dir.path().join(".edict.toml"), "").unwrap();
        std::fs::write(dir.path().join(".botbox.toml"), "").unwrap();
        std::fs::write(dir.path().join(".botbox.json"), "").unwrap();

        let found = find_config(dir.path()).unwrap();
        assert!(found.to_string_lossy().ends_with(".edict.toml"));
    }

    #[test]
    fn find_config_falls_back_to_legacy_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".botbox.toml"), "").unwrap();
        std::fs::write(dir.path().join(".botbox.json"), "").unwrap();

        let found = find_config(dir.path()).unwrap();
        assert!(found.to_string_lossy().ends_with(".botbox.toml"));
    }

    #[test]
    fn find_config_falls_back_to_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".botbox.json"), "").unwrap();

        let found = find_config(dir.path()).unwrap();
        assert!(found.to_string_lossy().ends_with(".botbox.json"));
    }

    #[test]
    fn find_config_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_config(dir.path()).is_none());
    }

    // --- find_config_in_project tests ---

    #[test]
    fn find_config_in_project_root_toml_preferred() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".edict.toml"), "").unwrap();
        std::fs::write(dir.path().join(".botbox.json"), "").unwrap();

        let (path, config_dir) = find_config_in_project(dir.path()).unwrap();
        assert!(path.to_string_lossy().ends_with(".edict.toml"));
        assert_eq!(config_dir, dir.path());
    }

    #[test]
    fn find_config_in_project_ws_toml_beats_root_json() {
        // maw v2 scenario: stale .botbox.json at bare root, migrated .edict.toml in ws/default
        let dir = tempfile::tempdir().unwrap();
        let ws_default = dir.path().join("ws/default");
        std::fs::create_dir_all(&ws_default).unwrap();

        std::fs::write(dir.path().join(".botbox.json"), "").unwrap(); // stale root JSON
        std::fs::write(ws_default.join(".edict.toml"), "").unwrap(); // current ws/default TOML

        let (path, config_dir) = find_config_in_project(dir.path()).unwrap();
        assert!(
            path.to_string_lossy().ends_with(".edict.toml"),
            "ws/default TOML should beat stale root JSON, got: {path:?}"
        );
        assert_eq!(config_dir, ws_default);
    }

    #[test]
    fn find_config_in_project_legacy_toml_accepted() {
        // Pre-migration: .botbox.toml still present, no .edict.toml yet
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".botbox.toml"), "").unwrap();

        let (path, config_dir) = find_config_in_project(dir.path()).unwrap();
        assert!(path.to_string_lossy().ends_with(".botbox.toml"));
        assert_eq!(config_dir, dir.path());
    }

    #[test]
    fn find_config_in_project_root_json_fallback() {
        // Legacy single-workspace: root JSON only
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".botbox.json"), "").unwrap();

        let (path, config_dir) = find_config_in_project(dir.path()).unwrap();
        assert!(path.to_string_lossy().ends_with(".botbox.json"));
        assert_eq!(config_dir, dir.path());
    }

    #[test]
    fn find_config_in_project_ws_json_fallback() {
        // maw v2 with JSON not yet migrated
        let dir = tempfile::tempdir().unwrap();
        let ws_default = dir.path().join("ws/default");
        std::fs::create_dir_all(&ws_default).unwrap();
        std::fs::write(ws_default.join(".botbox.json"), "").unwrap();

        let (path, config_dir) = find_config_in_project(dir.path()).unwrap();
        assert!(path.to_string_lossy().ends_with(".botbox.json"));
        assert_eq!(config_dir, ws_default);
    }

    #[test]
    fn find_config_in_project_missing() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_config_in_project(dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no .edict.toml or .botbox.toml")
        );
    }

    #[test]
    fn to_toml_includes_comments() {
        let config = Config::parse_toml(
            r#"
version = "1.0.0"
[project]
name = "test"
[tools]
bones = true
"#,
        )
        .unwrap();
        let output = config.to_toml().unwrap();
        assert!(output.contains("#:schema https://raw.githubusercontent.com/bobisme/edict"));
        assert!(output.contains("# Edict project configuration"));
        assert!(output.contains("# Companion tools to enable"));
    }

    #[test]
    fn parse_toml_with_env_section() {
        let toml_str = r#"
version = "1.0.0"

[project]
name = "test"

[env]
CARGO_BUILD_JOBS = "2"
RUSTC_WRAPPER = "sccache"
"#;

        let config = Config::parse_toml(toml_str).unwrap();
        assert_eq!(config.env.len(), 2);
        assert_eq!(config.env["CARGO_BUILD_JOBS"], "2");
        assert_eq!(config.env["RUSTC_WRAPPER"], "sccache");
    }

    #[test]
    fn parse_toml_without_env_section() {
        let toml_str = r#"
version = "1.0.0"
[project]
name = "test"
"#;
        let config = Config::parse_toml(toml_str).unwrap();
        assert!(config.env.is_empty());
    }

    #[test]
    fn expand_env_value_dollar_var() {
        // Set a test var then expand it
        unsafe { std::env::set_var("EDICT_TEST_VAR", "/test/path"); }
        assert_eq!(expand_env_value("$EDICT_TEST_VAR/sub"), "/test/path/sub");
        assert_eq!(expand_env_value("${EDICT_TEST_VAR}/sub"), "/test/path/sub");
        unsafe { std::env::remove_var("EDICT_TEST_VAR"); }
    }

    #[test]
    fn expand_env_value_unset_var_preserved() {
        // Unset vars should be left as-is
        let result = expand_env_value("$EDICT_NONEXISTENT_VAR_12345");
        assert_eq!(result, "$EDICT_NONEXISTENT_VAR_12345");
        let result = expand_env_value("${EDICT_NONEXISTENT_VAR_12345}");
        assert_eq!(result, "${EDICT_NONEXISTENT_VAR_12345}");
    }

    #[test]
    fn expand_env_value_no_vars() {
        assert_eq!(expand_env_value("plain string"), "plain string");
        assert_eq!(expand_env_value("/usr/bin/sccache"), "/usr/bin/sccache");
    }

    #[test]
    fn resolved_env_expands_values() {
        unsafe { std::env::set_var("EDICT_TEST_HOME", "/home/test"); }
        let config = Config::parse_toml(r#"
version = "1.0.0"
[project]
name = "test"
[env]
SCCACHE_DIR = "$EDICT_TEST_HOME/.cache/sccache"
PLAIN = "no-vars"
"#).unwrap();
        let resolved = config.resolved_env();
        assert_eq!(resolved["SCCACHE_DIR"], "/home/test/.cache/sccache");
        assert_eq!(resolved["PLAIN"], "no-vars");
        unsafe { std::env::remove_var("EDICT_TEST_HOME"); }
    }
}
