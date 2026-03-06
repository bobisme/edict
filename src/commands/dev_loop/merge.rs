//! Merge protocol helpers.
//!
//! The merge protocol is primarily prompt-driven, but these helpers provide
//! the Rust-side implementation for mutex acquisition, rebase, and merge.

use std::thread;
use std::time::{Duration, Instant};

use crate::subprocess::Tool;

/// Acquire the merge mutex (workspace://project/default claim).
///
/// Retries with exponential backoff + jitter: 2s, 4s, 8s, 15s.
/// Returns Ok(()) on success, Err if timeout reached.
pub fn acquire_merge_mutex(
    agent: &str,
    project: &str,
    ws: &str,
    bead_id: &str,
    timeout_secs: u64,
) -> anyhow::Result<()> {
    let delays = [2u64, 4, 8, 15];
    let start = Instant::now();
    let memo = format!("merging {ws} for {bead_id}");
    let timeout = Duration::from_secs(timeout_secs);
    let mut attempt = 0usize;

    loop {
        let result = Tool::new("rite")
            .args(&[
                "claims",
                "stake",
                "--agent",
                agent,
                &format!("workspace://{project}/default"),
                "--ttl",
                "120",
                "-m",
                &memo,
            ])
            .run();

        match result {
            Ok(output) if output.success() => return Ok(()),
            _ => {
                if start.elapsed() >= timeout {
                    anyhow::bail!(
                        "merge mutex timeout after {}s — another agent holds workspace://{}/default",
                        timeout_secs,
                        project
                    );
                }

                // Exponential backoff with jitter, capped at 15s
                let base_delay = delays[attempt.min(delays.len() - 1)];
                let jitter_range = (base_delay as f64 * 0.3) as u64;
                let jitter = if jitter_range > 0 {
                    (attempt as u64 * 7 + 3) % (2 * jitter_range + 1)
                } else {
                    0
                };
                let actual_delay = base_delay
                    .saturating_sub(jitter_range)
                    .saturating_add(jitter);

                attempt += 1;
                eprintln!(
                    "Merge mutex held by another agent, retrying in {actual_delay}s (attempt {attempt})"
                );

                // Check if a coord:merge message appeared (lock might be free)
                if let Ok(output) = Tool::new("rite")
                    .args(&[
                        "history",
                        project,
                        "-L",
                        "coord:merge",
                        "-n",
                        "1",
                        "--since",
                        "2 minutes ago",
                    ])
                    .run()
                    && output.success()
                    && !output.stdout.trim().is_empty()
                {
                    eprintln!("coord:merge detected — retrying immediately");
                    continue;
                }

                thread::sleep(Duration::from_secs(actual_delay));
            }
        }
    }
}

/// Release the merge mutex.
pub fn release_merge_mutex(agent: &str, project: &str) {
    let _ = Tool::new("rite")
        .args(&[
            "claims",
            "release",
            "--agent",
            agent,
            &format!("workspace://{project}/default"),
        ])
        .run();
}

/// Check merge readiness for a workspace.
pub fn check_merge(ws: &str) -> anyhow::Result<()> {
    Tool::new("maw")
        .args(&["ws", "merge", ws, "--check"])
        .run_ok()?;
    Ok(())
}

/// Merge a workspace into default (squash merge + destroy).
pub fn merge_workspace(ws: &str) -> anyhow::Result<()> {
    Tool::new("maw")
        .args(&["ws", "merge", ws, "--destroy"])
        .run_ok()?;
    Ok(())
}

/// Sync bones after merge.
///
/// bones is event-sourced — no sync step needed. This is a no-op retained
/// for call-site compatibility during migration.
pub fn sync_bones() {
    // bn is event-sourced, no sync required
}
