//! Worker dispatch helpers.
//!
//! The actual dispatch logic is prompt-driven (Claude decides what to dispatch).
//! These helpers support the dispatch infrastructure: workspace creation,
//! worker naming, claim staking, and vessel spawn.

use crate::subprocess::Tool;

/// Create a workspace named after the bone and return its name.
pub fn create_workspace(bone_id: &str, description: &str) -> anyhow::Result<String> {
    Tool::new("maw")
        .args(&[
            "ws",
            "create",
            bone_id,
            "--from",
            "main",
            "--description",
            description,
        ])
        .run_ok()?;
    Ok(bone_id.to_string())
}

/// Generate a random worker name suffix.
pub fn generate_worker_name() -> anyhow::Result<String> {
    let output = Tool::new("rite").arg("generate-name").run_ok()?;
    Ok(output.stdout.trim().to_string())
}

/// Stake a bone claim.
pub fn claim_bone(agent: &str, project: &str, bone_id: &str, memo: &str) -> anyhow::Result<()> {
    Tool::new("rite")
        .args(&[
            "claims",
            "stake",
            "--agent",
            agent,
            &format!("bone://{project}/{bone_id}"),
            "-m",
            memo,
        ])
        .run_ok()?;
    Ok(())
}

/// Stake a workspace claim.
pub fn claim_workspace(agent: &str, project: &str, ws: &str, memo: &str) -> anyhow::Result<()> {
    Tool::new("rite")
        .args(&[
            "claims",
            "stake",
            "--agent",
            agent,
            &format!("workspace://{project}/{ws}"),
            "-m",
            memo,
        ])
        .run_ok()?;
    Ok(())
}

/// Timeout in seconds for a given model or tier name.
pub fn model_timeout(model: &str) -> u64 {
    match model {
        "strong" | "opus" => 900,
        "balanced" | "sonnet" => 600,
        "fast" | "haiku" => 300,
        _ => 600,
    }
}
