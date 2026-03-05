use std::fs;
use std::path::{Path, PathBuf};

use crate::subprocess::Tool;

/// Journal for recording dev-loop iteration history.
///
/// Stored at `~/.cache/edict/projects/<slug>/dev-loop.txt` (XDG-compliant).
pub struct Journal {
    path: PathBuf,
}

/// Data from a previous iteration journal entry.
pub struct LastIteration {
    pub content: String,
    pub age: String,
}

impl Journal {
    /// Create a new journal for the given project root.
    pub fn new(project_root: &Path) -> Self {
        let cache_dir = get_cache_dir(project_root);
        Self {
            path: cache_dir.join("dev-loop.txt"),
        }
    }

    /// Truncate the journal at the start of a new loop session.
    pub fn truncate(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if self.path.exists() {
            let _ = fs::write(&self.path, "");
        }
    }

    /// Append an entry to the journal with a timestamp header.
    pub fn append(&self, entry: &str) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let timestamp = chrono_timestamp();
        let change_id = get_jj_change_id();

        let mut header = format!("\n--- {timestamp}");
        if let Some(cid) = change_id {
            header.push_str(&format!(" | git:{cid}"));
        }
        header.push_str(" ---\n");

        let content = format!("{header}{}\n", entry.trim());

        if let Err(e) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(content.as_bytes())
            })
        {
            eprintln!("Warning: Failed to append to journal: {e}");
        }
    }

    /// Read the last iteration summary from the journal, with age info.
    pub fn read_last(&self) -> Option<LastIteration> {
        if !self.path.exists() {
            return None;
        }

        let content = fs::read_to_string(&self.path).ok()?;
        if content.trim().is_empty() {
            return None;
        }

        let metadata = fs::metadata(&self.path).ok()?;
        let modified = metadata.modified().ok()?;
        let age = format_age(modified);

        Some(LastIteration {
            content: content.trim().to_string(),
            age,
        })
    }
}

/// Get the XDG-compliant cache directory for edict.
fn get_cache_dir(project_root: &Path) -> PathBuf {
    let base = if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else if cfg!(target_os = "macos") {
        dirs_home().join("Library/Caches")
    } else {
        dirs_home().join(".cache")
    };

    // Slugify the project path
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let slug = canonical
        .to_string_lossy()
        .replace(['/', '\\'], "-")
        .trim_start_matches('-')
        .to_string();

    base.join("edict").join("projects").join(slug)
}

/// Get the home directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Get the current git HEAD short hash from the default workspace.
fn get_jj_change_id() -> Option<String> {
    let output = Tool::new("git")
        .args(&["rev-parse", "--short", "HEAD"])
        .in_workspace("default")
        .ok()?
        .run()
        .ok()?;

    if output.success() {
        let id = output.stdout.trim().to_string();
        if id.is_empty() { None } else { Some(id) }
    } else {
        None
    }
}

/// Generate an ISO 8601 timestamp.
fn chrono_timestamp() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple UTC timestamp (no external crate needed)
    let secs = now.as_secs();
    let (hours, remainder) = (secs / 3600, secs % 3600);
    let (days_since_epoch, hour) = (hours / 24, hours % 24);
    let (mins, secs_in_min) = (remainder / 60, remainder % 60);

    // Compute date from days since epoch (1970-01-01)
    let (year, month, day) = days_to_date(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{mins:02}:{secs_in_min:02}Z")
}

/// Convert days since epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Civil date algorithm from Howard Hinnant
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}

/// Format age of a file modification time as human-readable string.
fn format_age(modified: std::time::SystemTime) -> String {
    let elapsed = modified.elapsed().unwrap_or_default();
    let mins = elapsed.as_secs() / 60;
    let hours = mins / 60;

    if hours > 0 {
        format!("{hours}h ago")
    } else {
        format!("{mins}m ago")
    }
}
