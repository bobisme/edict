//! Mission orchestration helpers (Level 4).
//!
//! Missions are large tasks decomposed into child bones dispatched to parallel workers.
//! The orchestration logic is primarily prompt-driven; these helpers support
//! checkpoint state management and mission lifecycle tracking.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Mission checkpoint state, serialized to cache dir for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionCheckpoint {
    pub mission_id: String,
    pub total_children: u32,
    pub closed: u32,
    pub in_progress: u32,
    pub blocked: u32,
    pub open: u32,
    pub dispatched_workers: Vec<DispatchedWorker>,
    pub last_checkpoint_time: String,
}

/// Record of a dispatched worker for cross-referencing with botty list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchedWorker {
    pub worker_name: String,
    pub bead_id: String,
    pub workspace: String,
    pub model: String,
}

impl MissionCheckpoint {
    /// Load checkpoint from cache file.
    pub fn load(mission_id: &str) -> Option<Self> {
        let path = checkpoint_path(mission_id);
        let contents = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    /// Save checkpoint to cache file.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = checkpoint_path(&self.mission_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        Ok(())
    }

    /// Remove checkpoint file after mission completes.
    pub fn remove(mission_id: &str) {
        let path = checkpoint_path(mission_id);
        let _ = fs::remove_file(path);
    }

    /// Check if all children are done.
    pub fn is_complete(&self) -> bool {
        self.closed == self.total_children
    }

    /// Check if the mission is stuck (all remaining bones blocked, no workers alive).
    pub fn is_stuck(&self) -> bool {
        self.in_progress == 0 && self.blocked > 0 && self.open == 0
    }
}

/// Get the cache path for a mission checkpoint.
fn checkpoint_path(mission_id: &str) -> PathBuf {
    let cache_base = if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join("Library/Caches")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".cache")
    };

    cache_base
        .join("edict")
        .join("missions")
        .join(format!("{mission_id}.json"))
}
