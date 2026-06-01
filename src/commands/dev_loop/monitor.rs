//! Worker monitoring helpers.
//!
//! The monitoring logic is mostly prompt-driven. These helpers provide
//! utilities for checking worker status and detecting dead workers.

use crate::subprocess::Tool;

/// Worker info from vessel list.
pub struct WorkerInfo {
    pub name: String,
    pub status: String,
}

/// List active workers that belong to this agent (hierarchical naming: agent/suffix).
pub fn list_child_workers(agent: &str) -> Vec<WorkerInfo> {
    let output = match Tool::new("vessel")
        .args(&["list", "--format", "json"])
        .run()
    {
        Ok(o) if o.success() => o,
        _ => return Vec::new(),
    };

    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).unwrap_or_default();
    let agents = parsed["agents"].as_array().cloned().unwrap_or_default();
    let prefix = format!("{agent}/");

    agents
        .iter()
        .filter_map(|a| {
            let name = a["id"].as_str().or(a["name"].as_str())?;
            if name.starts_with(&prefix) {
                Some(WorkerInfo {
                    name: name.to_string(),
                    status: a["status"].as_str().unwrap_or("running").to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Check if a specific worker is still alive.
pub fn is_worker_alive(agent: &str, worker_name: &str) -> bool {
    let workers = list_child_workers(agent);
    workers.iter().any(|w| w.name == worker_name)
}

/// Kill a specific worker by name.
pub fn kill_worker(name: &str) -> anyhow::Result<()> {
    Tool::new("vessel").args(&["kill", name]).run_ok()?;
    Ok(())
}
