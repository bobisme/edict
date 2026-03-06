//! Release check helpers.
//!
//! Scans commits since last tag for feat:/fix: prefixes to determine
//! if a release is needed. The actual version bumping and tagging is
//! prompt-driven.

use crate::subprocess::Tool;

/// Check if there are unreleased user-visible commits (feat: or fix:).
pub fn has_unreleased_changes() -> bool {
    // Get the latest tag
    let tag_output = Tool::new("git")
        .args(&["describe", "--tags", "--abbrev=0"])
        .in_workspace("default")
        .ok()
        .and_then(|t| t.run().ok());

    let range = match tag_output {
        Some(ref o) if o.success() => {
            let tag = o.stdout.trim();
            format!("{tag}..HEAD")
        }
        _ => "HEAD~20..HEAD".to_string(),
    };

    let output = Tool::new("git")
        .args(&["log", "--oneline", &range])
        .in_workspace("default")
        .ok()
        .and_then(|t| t.run().ok());

    match output {
        Some(o) if o.success() => o
            .stdout
            .lines()
            .any(|line| {
                // git log --oneline: "<hash> <message>"
                let msg = line.split_once(' ').map_or(line, |(_, m)| m);
                msg.starts_with("feat:") || msg.starts_with("fix:")
            }),
        _ => false,
    }
}

/// Acquire the release mutex.
pub fn acquire_release_mutex(agent: &str, project: &str) -> anyhow::Result<()> {
    Tool::new("rite")
        .args(&[
            "claims",
            "stake",
            "--agent",
            agent,
            &format!("release://{project}"),
            "--ttl",
            "120",
            "-m",
            "checking release",
        ])
        .run_ok()?;
    Ok(())
}

/// Release the release mutex.
pub fn release_release_mutex(agent: &str, project: &str) {
    let _ = Tool::new("rite")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("release://{project}"),
        ])
        .run();
}
