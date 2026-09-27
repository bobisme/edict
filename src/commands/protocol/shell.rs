//! Shell-safe primitives for protocol guidance rendering.
//!
//! Single-quote escaping, identifier validation, and command builder helpers.
//! The renderer layer composes these rather than duplicating quoting logic.

use std::fmt::Write;

/// Escape a string for safe inclusion in a single-quoted shell argument.
///
/// The POSIX approach: wrap in single quotes, and for any embedded single
/// quote, end the current quoting, insert an escaped single quote, and
/// restart quoting: `'` → `'\''`.
///
/// Returns the string with surrounding single quotes.
#[must_use]
pub fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Validate a bone ID (e.g., `bd-3cqv`, `bn-m80`).
///
/// Bone ID prefixes vary by project, so we validate the format
/// (short alphanumeric with hyphens) without hardcoding a prefix.
///
/// # Errors
///
/// Returns `Err` if the ID is empty or does not match the expected format.
pub fn validate_bone_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty() {
        return Err(ValidationError::Empty("bone ID"));
    }
    let valid = id.len() <= 20
        && id.contains('-')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !valid {
        return Err(ValidationError::InvalidFormat {
            field: "bone ID",
            value: id.to_string(),
            expected: "<prefix>-[a-z0-9]+",
        });
    }
    Ok(())
}

/// Validate a workspace name.
///
/// # Errors
///
/// Returns `Err` if the name is empty, too long, or does not match the expected format.
pub fn validate_workspace_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::Empty("workspace name"));
    }
    if name.len() > 64 {
        return Err(ValidationError::TooLong {
            field: "workspace name",
            max: 64,
            actual: name.len(),
        });
    }
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !valid {
        return Err(ValidationError::InvalidFormat {
            field: "workspace name",
            value: name.to_string(),
            expected: "[a-z0-9][a-z0-9-]*, max 64 chars",
        });
    }
    Ok(())
}

/// Validate an identifier (agent name, project name).
/// Must be non-empty and contain no shell metacharacters.
///
/// # Errors
///
/// Returns `Err` if the value is empty or contains shell metacharacters.
pub fn validate_identifier(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError::Empty(field));
    }
    let has_unsafe = value.chars().any(|c| {
        matches!(
            c,
            ' ' | '\t'
                | '\n'
                | '\r'
                | '\''
                | '"'
                | '`'
                | '$'
                | '\\'
                | '!'
                | '&'
                | '|'
                | ';'
                | '('
                | ')'
                | '{'
                | '}'
                | '<'
                | '>'
                | '*'
                | '?'
                | '['
                | ']'
                | '#'
                | '~'
                | '\0'
        )
    });
    if has_unsafe {
        return Err(ValidationError::UnsafeChars {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

/// Validation error for shell-rendered values.
#[derive(Debug, Clone)]
pub enum ValidationError {
    Empty(&'static str),
    TooLong {
        field: &'static str,
        max: usize,
        actual: usize,
    },
    InvalidFormat {
        field: &'static str,
        value: String,
        expected: &'static str,
    },
    UnsafeChars {
        field: &'static str,
        value: String,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "{field} cannot be empty"),
            Self::TooLong {
                field, max, actual, ..
            } => {
                write!(f, "{field} too long ({actual} chars, max {max})")
            }
            Self::InvalidFormat {
                field,
                value,
                expected,
            } => {
                write!(f, "invalid {field} '{value}', expected {expected}")
            }
            Self::UnsafeChars { field, value } => {
                write!(f, "{field} '{value}' contains shell metacharacters")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate a review ID (e.g., `cr-2rnh`).
///
/// # Errors
///
/// Returns `Err` if the ID is empty or does not match the `cr-[a-z0-9]+` format.
pub fn validate_review_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty() {
        return Err(ValidationError::Empty("review ID"));
    }
    let valid =
        id.starts_with("cr-") && id.len() > 3 && id[3..].chars().all(|c| c.is_ascii_alphanumeric());
    if !valid {
        return Err(ValidationError::InvalidFormat {
            field: "review ID",
            value: id.to_string(),
            expected: "cr-[a-z0-9]+",
        });
    }
    Ok(())
}

/// Ensure a structural value is safe for direct shell interpolation.
///
/// Structural values (bone IDs, workspace names, project names, statuses, labels)
/// are expected to be pre-validated identifiers. As defense-in-depth, if a value
/// contains shell metacharacters, it is escaped rather than interpolated raw.
fn safe_ident(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
    {
        std::borrow::Cow::Borrowed(value)
    } else {
        std::borrow::Cow::Owned(shell_escape(value))
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource<'a> {
    Main,
    Change(&'a str),
}

impl WorkspaceSource<'_> {
    fn write_shell_args(self, cmd: &mut String) {
        match self {
            Self::Main => cmd.push_str(" --from main"),
            Self::Change(change_id) => {
                cmd.push_str(" --change ");
                cmd.push_str(&safe_ident(change_id));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeTarget<'a> {
    Default,
    Change(&'a str),
}

impl MergeTarget<'_> {
    fn shell_value(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Default => std::borrow::Cow::Borrowed("default"),
            Self::Change(change_id) => std::borrow::Cow::Owned(safe_ident(change_id).into_owned()),
        }
    }
}

// --- Command builders ---
// These produce shell-safe command strings. All dynamic values are validated
// or escaped before inclusion. Structural identifiers pass through safe_ident()
// for defense-in-depth against unvalidated callers.

/// Build: `rite claims stake --agent <agent> "bone://<project>/<id>" -m "<memo>"`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn claims_stake_cmd(agent: &str, uri: &str, memo: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    let mut cmd = String::new();
    write!(
        cmd,
        "rite claims stake --agent {} {}",
        agent_safe,
        shell_escape(uri)
    )
    .expect("writing to a String is infallible");
    if !memo.is_empty() {
        write!(cmd, " -m {}", shell_escape(memo)).expect("writing to a String is infallible");
    }
    cmd
}

/// Build: `rite claims release --agent <agent> "<uri>"`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[allow(dead_code)]
#[must_use]
pub fn claims_release_cmd(agent: &str, uri: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!(
        "rite claims release --agent {} {}",
        agent_safe,
        shell_escape(uri)
    )
}

/// Build: `rite claims release --agent <agent> --all`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn claims_release_all_cmd(agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!("rite claims release --agent {agent_safe} --all")
}

/// Build: `rite send --agent <agent> <project> '<message>' -L <label>`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn rite_send_cmd(agent: &str, project: &str, message: &str, label: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

    // Validate project name before use
    if validate_identifier("project", project).is_err() {
        // If validation fails, force escaping instead of raw interpolation
        let mut cmd = String::new();
        write!(
            cmd,
            "rite send --agent {} {} {}",
            agent_safe,
            shell_escape(project),
            shell_escape(message)
        )
        .expect("writing to a String is infallible");
        if !label.is_empty() {
            write!(cmd, " -L {}", shell_escape(label)).expect("writing to a String is infallible");
        }
        return cmd;
    }

    let mut cmd = String::new();
    write!(
        cmd,
        "rite send --agent {} {} {}",
        agent_safe,
        safe_ident(project),
        shell_escape(message)
    )
    .expect("writing to a String is infallible");
    if !label.is_empty() {
        // Apply same validate+escape fallback as project parameter
        if validate_identifier("label", label).is_ok() {
            write!(cmd, " -L {}", safe_ident(label)).expect("writing to a String is infallible");
        } else {
            write!(cmd, " -L {}", shell_escape(label)).expect("writing to a String is infallible");
        }
    }
    cmd
}

/// Advice that turns an announced review request into an answered one.
///
/// The announce step above is a plain command, so the id of the message it sent
/// is read back from history rather than captured. The wait is what stops the
/// requester from asking again.
#[must_use]
pub fn review_wait_advice(agent: &str, project: &str) -> String {
    let agent_safe = safe_ident(agent);
    let project_safe = safe_ident(project);
    let timeout = crate::reply::DEFAULT_WAIT_TIMEOUT;
    format!(
        "Then block on the verdict instead of asking again: \
         req=$(rite history {project_safe} --from {agent_safe} -n 1 --format json | jq -r .last_id) \
         && rite wait --agent {agent_safe} --reply-to \"$req\" -t {timeout} --format json. \
         Exit 0 = answered, exit 1 = escalate with -L task-blocked (never re-announce), \
         exit 2 = wrong id."
    )
}

/// Build: `maw exec default -- bn do <id>`
#[allow(dead_code)]
#[must_use]
pub fn bn_do_cmd(bone_id: &str) -> String {
    // Validate bone_id before use - escape if validation fails
    let bone_id_safe = if validate_bone_id(bone_id).is_ok() {
        safe_ident(bone_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(bone_id))
    };

    format!("maw exec default -- bn do {bone_id_safe}")
}

/// Build: `maw exec default -- bn bone comment add <id> '<message>'`
#[allow(dead_code)]
#[must_use]
pub fn bn_comment_cmd(bone_id: &str, message: &str) -> String {
    // Validate bone_id before use
    let bone_id_safe = if validate_bone_id(bone_id).is_ok() {
        safe_ident(bone_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(bone_id))
    };

    format!(
        "maw exec default -- bn bone comment add {} {}",
        bone_id_safe,
        shell_escape(message)
    )
}

/// Build: `maw exec default -- bn done <id> --reason '<reason>'`
#[must_use]
pub fn bn_done_cmd(bone_id: &str, reason: &str) -> String {
    // Validate bone_id before use
    let bone_id_safe = if validate_bone_id(bone_id).is_ok() {
        safe_ident(bone_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(bone_id))
    };

    let mut cmd = format!("maw exec default -- bn done {bone_id_safe}");
    if !reason.is_empty() {
        write!(cmd, " --reason {}", shell_escape(reason))
            .expect("writing to a String is infallible");
    }
    cmd
}

/// Build: `maw ws create <name> --from main --description "..."`
#[must_use]
pub fn ws_create_cmd(name: &str, description: &str, source: WorkspaceSource<'_>) -> String {
    let workspace_safe = if validate_workspace_name(name).is_ok() {
        safe_ident(name)
    } else {
        std::borrow::Cow::Owned(shell_escape(name))
    };

    let mut cmd = format!(
        "maw ws create {} --description {}",
        workspace_safe,
        shell_escape(description)
    );
    source.write_shell_args(&mut cmd);
    cmd
}

/// Build: `maw ws merge <ws> --into <target> --check --format json`
#[must_use]
pub fn ws_merge_check_cmd(workspace: &str, target: MergeTarget<'_>) -> String {
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let target_safe = target.shell_value();
    format!("maw ws merge {workspace_safe} --into {target_safe} --check --format json")
}

/// Build: `maw ws merge <ws> --into <target> --destroy --message <msg>`
///
/// `message` is required — maw enforces explicit commit messages.
/// Use conventional commit prefix: `feat:`, `fix:`, `chore:`, etc.
#[must_use]
pub fn ws_merge_cmd(workspace: &str, target: MergeTarget<'_>, message: &str) -> String {
    // Validate workspace name before use
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let target_safe = target.shell_value();

    format!(
        "maw ws merge {} --into {} --destroy --message {}",
        workspace_safe,
        target_safe,
        shell_escape(message)
    )
}

/// Build the steps that record a Seal review in its workspace before the merge.
///
/// Seal keeps each review as an event log under `.seal/reviews/<id>/` in the
/// workspace that created it. The log reaches trunk only when it is committed
/// there, so these steps must run before `maw ws merge --destroy`:
///
/// 1. Refuse when anything outside the review log is uncommitted. `maw ws merge`
///    captures uncommitted additions and deletions too, but the approval covers
///    only a commit, so those changes would land unreviewed.
/// 2. `seal reviews mark-merged` while HEAD is still the approved commit. Run
///    after the log commit, a branch-anchored review counts that commit as
///    unreviewed and refuses.
/// 3. Stage the log and commit it alone. The commit is skipped when nothing is
///    staged, so a retry after a failed merge is safe.
///
/// Only the agent that runs the merge may emit these. A merged review no longer
/// satisfies the merge gate, so marking it earlier blocks the merge.
///
/// The merge step itself must repeat the check: see [`ws_merge_reviewed_cmd`].
#[must_use]
pub fn seal_record_cmds(workspace: &str, review_id: &str) -> Vec<String> {
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };
    let (review_id_safe, log_path) = if validate_review_id(review_id).is_ok() {
        (safe_ident(review_id), format!(".seal/reviews/{review_id}"))
    } else {
        (
            std::borrow::Cow::Owned(shell_escape(review_id)),
            shell_escape(&format!(".seal/reviews/{review_id}")),
        )
    };
    let message = shell_escape(&format!("chore: seal review {review_id}"));

    vec![
        seal_clean_check_cmd(workspace, review_id),
        format!("maw exec {workspace_safe} -- seal reviews mark-merged {review_id_safe}"),
        format!("maw exec {workspace_safe} -- git add {log_path}"),
        format!(
            "maw exec {workspace_safe} -- git diff --cached --quiet -- {log_path} || \
             maw exec {workspace_safe} -- git commit -m {message} -- {log_path}"
        ),
    ]
}

/// Build a check that fails when anything outside the review log is uncommitted.
///
/// `maw ws merge` merges the live workspace, including uncommitted additions
/// and deletions, but a Seal approval covers only committed history. The check
/// keeps the merged tree equal to the reviewed commits plus the review log.
#[must_use]
pub fn seal_clean_check_cmd(workspace: &str, review_id: &str) -> String {
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };
    let exclude = shell_escape(&format!(":(exclude).seal/reviews/{review_id}"));
    // `out=$(...)` takes the exit status of `git status`, so a failed
    // inspection stops here instead of reading as an empty, clean tree.
    format!(
        "{{ out=$(maw exec {workspace_safe} -- git status --porcelain \
         --untracked-files=all -- . {exclude}) && test -z \"$out\" || {{ echo 'uncommitted \
         changes outside the review log were never reviewed, or the workspace could not be \
         inspected: commit them and get a fresh LGTM' >&2; false; }}; }}"
    )
}

/// Build the merge step for a reviewed workspace: the clean check, then the
/// merge, in one shell command.
///
/// The record steps check first, but `mark-merged`, the log commit, and any
/// hook they run can leave files behind. Chaining the check onto the merge
/// narrows the window to the time maw takes to snapshot the workspace. Only a
/// maw merge of committed content can close it.
#[must_use]
pub fn ws_merge_reviewed_cmd(
    workspace: &str,
    review_id: &str,
    target: MergeTarget<'_>,
    message: &str,
) -> String {
    format!(
        "{} && {}",
        seal_clean_check_cmd(workspace, review_id),
        ws_merge_cmd(workspace, target, message)
    )
}

/// Build: `maw exec <ws> -- seal reviews create --agent <agent> --title '<bone-id>: <title>' --reviewers <reviewers>`
///
/// The `<bone-id>: ` prefix is applied HERE rather than by callers. A review is
/// bound to its bone only by that title convention, so a create command that
/// omits it produces a review the gate can never find: it can be fully approved
/// while `protocol finish`/`merge` still report `NeedsReview` — and the remedy
/// printed next to that is `--force`, which skips the gate outright. Owning the
/// prefix at the single choke point makes an unmatchable title unrepresentable
/// instead of relying on three call sites to remember it.
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn seal_create_cmd(
    workspace: &str,
    agent: &str,
    bone_id: &str,
    title: &str,
    reviewers: &str,
) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

    let title = super::review_select::scoped_title(bone_id, title);

    // Validate workspace and reviewers before use
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let reviewers_safe = if validate_identifier("reviewers", reviewers).is_ok() {
        safe_ident(reviewers)
    } else {
        std::borrow::Cow::Owned(shell_escape(reviewers))
    };

    format!(
        "maw exec {} -- seal reviews create --agent {} --title {} --reviewers {}",
        workspace_safe,
        agent_safe,
        shell_escape(&title),
        reviewers_safe
    )
}

/// Build: `maw exec <ws> -- seal reviews request <id> --reviewers <reviewers> --agent <agent>`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn seal_request_cmd(workspace: &str, review_id: &str, reviewers: &str, agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

    // Validate all identifiers before use
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let review_id_safe = if validate_review_id(review_id).is_ok() {
        safe_ident(review_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(review_id))
    };

    let reviewers_safe = if validate_identifier("reviewers", reviewers).is_ok() {
        safe_ident(reviewers)
    } else {
        std::borrow::Cow::Owned(shell_escape(reviewers))
    };

    format!(
        "maw exec {workspace_safe} -- seal reviews request {review_id_safe} --reviewers {reviewers_safe} --agent {agent_safe}"
    )
}

/// Build: `maw exec <ws> -- seal reviews retarget <id> --agent <agent>`
///
/// Moves an existing review's target commit to the workspace's current HEAD
/// and clears votes so the next round requires fresh approval. Must run
/// before re-requesting review on new commits — `seal reviews request` alone
/// leaves the review's target commit pinned to the old anchor (seal 0.29).
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn seal_retarget_cmd(workspace: &str, review_id: &str, agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let review_id_safe = if validate_review_id(review_id).is_ok() {
        safe_ident(review_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(review_id))
    };

    format!(
        "maw exec {workspace_safe} -- seal reviews retarget {review_id_safe} --agent {agent_safe}"
    )
}

/// Build: `maw exec <ws> -- seal review <id>`
#[must_use]
pub fn seal_show_cmd(workspace: &str, review_id: &str) -> String {
    // Validate workspace and review_id before use
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    let review_id_safe = if validate_review_id(review_id).is_ok() {
        safe_ident(review_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(review_id))
    };

    format!("maw exec {workspace_safe} -- seal review {review_id_safe}")
}

/// Build: `rite statuses clear --agent <agent>`
///
/// # Panics
///
/// Panics if `agent` is not a valid identifier.
#[must_use]
pub fn rite_statuses_clear_cmd(agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!("rite statuses clear --agent {agent_safe}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record steps run through `sh -c` in --execute mode. Every step,
    /// including the nested quoting of the dirty-tree check, must parse.
    #[test]
    fn seal_record_cmds_are_valid_shell() {
        for id in ["cr-123", "cr-bad'id"] {
            let mut steps = seal_record_cmds("frost-castle", id);
            steps.push(ws_merge_reviewed_cmd(
                "frost-castle",
                id,
                MergeTarget::Default,
                "feat: it's done",
            ));
            assert!(steps[0].contains("git status --porcelain --untracked-files=all"));
            assert!(steps[0].contains("&& test -z \"$out\""));
            for step in &steps {
                let status = std::process::Command::new("sh")
                    .args(["-n", "-c", step])
                    .status()
                    .expect("sh runs");
                assert!(status.success(), "sh -n rejects: {step}");
            }
        }
    }

    /// The clean check is a security gate: a `git status` that fails with no
    /// output must stop the merge, not read as a clean tree. A fake `maw` on
    /// PATH fails or passes the status call and records whether the merge ran.
    #[cfg(unix)]
    #[test]
    fn reviewed_merge_fails_closed_when_status_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("edict-fake-maw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("merged");
        let fake = dir.join("maw");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  exec) exit \"$FAKE_STATUS_RC\" ;;\n  ws) touch '{}' ;;\nesac\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cmd = ws_merge_reviewed_cmd("frost-castle", "cr-123", MergeTarget::Default, "feat: x");
        let path = format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let merged_with_status = |rc: &str| {
            let _ = std::fs::remove_file(&marker);
            let status = std::process::Command::new("sh")
                .args(["-c", &cmd])
                .env("PATH", &path)
                .env("FAKE_STATUS_RC", rc)
                .stderr(std::process::Stdio::null())
                .status()
                .expect("sh runs");
            (status.success(), marker.exists())
        };

        assert_eq!(
            merged_with_status("1"),
            (false, false),
            "failed status must not merge"
        );
        assert_eq!(
            merged_with_status("0"),
            (true, true),
            "clean status must merge"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- shell_escape tests ---

    #[test]
    fn escape_empty() {
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn escape_with_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn escape_single_quotes() {
        assert_eq!(shell_escape("it's here"), "'it'\\''s here'");
    }

    #[test]
    fn escape_double_quotes() {
        assert_eq!(shell_escape(r#"say "hi""#), r#"'say "hi"'"#);
    }

    #[test]
    fn escape_backslashes() {
        assert_eq!(shell_escape(r"path\to\file"), r"'path\to\file'");
    }

    #[test]
    fn escape_newlines() {
        assert_eq!(shell_escape("line1\nline2"), "'line1\nline2'");
    }

    #[test]
    fn escape_dollar_variables() {
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
    }

    #[test]
    fn escape_backticks() {
        assert_eq!(shell_escape("`whoami`"), "'`whoami`'");
    }

    #[test]
    fn escape_unicode() {
        assert_eq!(shell_escape("hello 🌍"), "'hello 🌍'");
    }

    #[test]
    fn escape_multiple_single_quotes() {
        assert_eq!(shell_escape("it's Bob's"), "'it'\\''s Bob'\\''s'");
    }

    #[test]
    fn escape_all_metacharacters() {
        assert_eq!(shell_escape("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    // --- validate_bone_id tests ---

    #[test]
    fn valid_bone_id() {
        assert!(validate_bone_id("bd-3cqv").is_ok());
        assert!(validate_bone_id("bd-abc123").is_ok());
        assert!(validate_bone_id("bd-a").is_ok());
        // Other project prefixes
        assert!(validate_bone_id("bn-m80").is_ok());
        assert!(validate_bone_id("bm-xyz").is_ok());
        assert!(validate_bone_id("xx-3cqv").is_ok());
    }

    #[test]
    fn invalid_bone_id_empty() {
        assert!(validate_bone_id("").is_err());
    }

    #[test]
    fn invalid_bone_id_no_hyphen() {
        assert!(validate_bone_id("3cqv").is_err());
        assert!(validate_bone_id("abcdef").is_err());
    }

    #[test]
    fn invalid_bone_id_special_chars() {
        assert!(validate_bone_id("bd-abc def").is_err());
        assert!(validate_bone_id("bd-abc;rm").is_err());
        assert!(validate_bone_id("bd-abc/def").is_err());
    }

    // --- validate_review_id tests ---

    #[test]
    fn valid_review_id() {
        assert!(validate_review_id("cr-2rnh").is_ok());
        assert!(validate_review_id("cr-abc123").is_ok());
        assert!(validate_review_id("cr-a").is_ok());
    }

    #[test]
    fn invalid_review_id_empty() {
        assert!(validate_review_id("").is_err());
    }

    #[test]
    fn invalid_review_id_no_prefix() {
        assert!(validate_review_id("2rnh").is_err());
        assert!(validate_review_id("bd-3cqv").is_err());
    }

    #[test]
    fn invalid_review_id_special_chars() {
        assert!(validate_review_id("cr-abc-def").is_err());
        assert!(validate_review_id("cr-").is_err());
    }

    // --- safe_ident tests ---

    #[test]
    fn safe_ident_passes_clean_values() {
        assert_eq!(safe_ident("bd-3cqv").as_ref(), "bd-3cqv");
        assert_eq!(safe_ident("frost-castle").as_ref(), "frost-castle");
        assert_eq!(safe_ident("in_progress").as_ref(), "in_progress");
        assert_eq!(safe_ident("edict-dev").as_ref(), "edict-dev");
    }

    #[test]
    fn safe_ident_escapes_unsafe_values() {
        // Spaces get escaped
        assert_eq!(safe_ident("bad name").as_ref(), "'bad name'");
        // Shell metacharacters get escaped
        assert_eq!(safe_ident("$(rm -rf)").as_ref(), "'$(rm -rf)'");
        // Empty gets escaped
        assert_eq!(safe_ident("").as_ref(), "''");
    }

    // --- validate_workspace_name tests ---

    #[test]
    fn valid_workspace_names() {
        assert!(validate_workspace_name("default").is_ok());
        assert!(validate_workspace_name("frost-castle").is_ok());
        assert!(validate_workspace_name("a").is_ok());
        assert!(validate_workspace_name("ws-123-test").is_ok());
    }

    #[test]
    fn invalid_workspace_empty() {
        assert!(validate_workspace_name("").is_err());
    }

    #[test]
    fn invalid_workspace_starts_with_dash() {
        assert!(validate_workspace_name("-foo").is_err());
    }

    #[test]
    fn invalid_workspace_special_chars() {
        assert!(validate_workspace_name("ws name").is_err());
        assert!(validate_workspace_name("ws_name").is_err());
        assert!(validate_workspace_name("ws.name").is_err());
    }

    #[test]
    fn invalid_workspace_too_long() {
        let long_name: String = "a".repeat(65);
        assert!(validate_workspace_name(&long_name).is_err());
    }

    #[test]
    fn workspace_exactly_64_chars() {
        let name: String = "a".repeat(64);
        assert!(validate_workspace_name(&name).is_ok());
    }

    // --- validate_identifier tests ---

    #[test]
    fn valid_identifiers() {
        assert!(validate_identifier("agent", "edict-dev").is_ok());
        assert!(validate_identifier("project", "myproject").is_ok());
        assert!(validate_identifier("agent", "my-agent-123").is_ok());
    }

    #[test]
    fn invalid_identifier_empty() {
        assert!(validate_identifier("agent", "").is_err());
    }

    #[test]
    fn invalid_identifier_shell_metacharacters() {
        assert!(validate_identifier("agent", "foo bar").is_err());
        assert!(validate_identifier("agent", "foo;rm").is_err());
        assert!(validate_identifier("agent", "$(whoami)").is_err());
        assert!(validate_identifier("agent", "foo`bar`").is_err());
        assert!(validate_identifier("agent", "foo'bar").is_err());
        assert!(validate_identifier("agent", "foo\"bar").is_err());
        assert!(validate_identifier("agent", "a|b").is_err());
        assert!(validate_identifier("agent", "a&b").is_err());
    }

    // --- Command builder tests ---

    #[test]
    fn claims_stake_basic() {
        let cmd = claims_stake_cmd("crimson-storm", "bone://myproject/bd-abc", "bd-abc");
        assert_eq!(
            cmd,
            "rite claims stake --agent crimson-storm 'bone://myproject/bd-abc' -m 'bd-abc'"
        );
    }

    #[test]
    fn claims_stake_no_memo() {
        let cmd = claims_stake_cmd("crimson-storm", "bone://myproject/bd-abc", "");
        assert_eq!(
            cmd,
            "rite claims stake --agent crimson-storm 'bone://myproject/bd-abc'"
        );
    }

    #[test]
    fn claims_release_basic() {
        let cmd = claims_release_cmd("crimson-storm", "bone://myproject/bd-abc");
        assert_eq!(
            cmd,
            "rite claims release --agent crimson-storm 'bone://myproject/bd-abc'"
        );
    }

    #[test]
    fn claims_release_all() {
        let cmd = claims_release_all_cmd("crimson-storm");
        assert_eq!(cmd, "rite claims release --agent crimson-storm --all");
    }

    #[test]
    fn rite_send_basic() {
        let cmd = rite_send_cmd(
            "crimson-storm",
            "myproject",
            "Task claimed: bd-abc",
            "task-claim",
        );
        assert_eq!(
            cmd,
            "rite send --agent crimson-storm myproject 'Task claimed: bd-abc' -L task-claim"
        );
    }

    #[test]
    fn rite_send_with_quotes_in_message() {
        let cmd = rite_send_cmd("crimson-storm", "myproject", "it's done", "task-done");
        assert_eq!(
            cmd,
            "rite send --agent crimson-storm myproject 'it'\\''s done' -L task-done"
        );
    }

    #[test]
    fn rite_send_no_label() {
        let cmd = rite_send_cmd("crimson-storm", "myproject", "hello", "");
        assert_eq!(cmd, "rite send --agent crimson-storm myproject 'hello'");
    }

    #[test]
    fn bn_do_basic() {
        let cmd = bn_do_cmd("bd-abc");
        assert_eq!(cmd, "maw exec default -- bn do bd-abc");
    }

    #[test]
    fn bn_comment_with_escaping() {
        let cmd = bn_comment_cmd("bd-abc", "Started work in ws/frost-castle/");
        assert_eq!(
            cmd,
            "maw exec default -- bn bone comment add bd-abc 'Started work in ws/frost-castle/'"
        );
    }

    #[test]
    fn bn_done_basic() {
        let cmd = bn_done_cmd("bd-abc", "Completed");
        assert_eq!(
            cmd,
            "maw exec default -- bn done bd-abc --reason 'Completed'"
        );
    }

    #[test]
    fn bn_done_no_reason() {
        let cmd = bn_done_cmd("bd-abc", "");
        assert_eq!(cmd, "maw exec default -- bn done bd-abc");
    }

    #[test]
    fn ws_create_from_main() {
        let cmd = ws_create_cmd("bn-1abc", "Fix login bug", WorkspaceSource::Main);
        assert_eq!(
            cmd,
            "maw ws create bn-1abc --description 'Fix login bug' --from main"
        );
    }

    #[test]
    fn ws_create_for_change() {
        let cmd = ws_create_cmd("bn-2def", "Add OAuth", WorkspaceSource::Change("ch-123"));
        assert_eq!(
            cmd,
            "maw ws create bn-2def --description 'Add OAuth' --change ch-123"
        );
    }

    #[test]
    fn ws_merge_check_default_target() {
        let cmd = ws_merge_check_cmd("frost-castle", MergeTarget::Default);
        assert_eq!(
            cmd,
            "maw ws merge frost-castle --into default --check --format json"
        );
    }

    #[test]
    fn ws_merge_with_default_target() {
        let cmd = ws_merge_cmd("frost-castle", MergeTarget::Default, "feat: add login flow");
        assert_eq!(
            cmd,
            "maw ws merge frost-castle --into default --destroy --message 'feat: add login flow'"
        );
    }

    #[test]
    fn ws_merge_with_change_target() {
        let cmd = ws_merge_cmd(
            "frost-castle",
            MergeTarget::Change("ch-123"),
            "feat: add login flow",
        );
        assert_eq!(
            cmd,
            "maw ws merge frost-castle --into ch-123 --destroy --message 'feat: add login flow'"
        );
    }

    #[test]
    fn seal_create_with_escaping() {
        let cmd = seal_create_cmd(
            "frost-castle",
            "crimson-storm",
            "bn-24r",
            "feat: add login",
            "myproject-security",
        );
        assert_eq!(
            cmd,
            "maw exec frost-castle -- seal reviews create --agent crimson-storm --title 'bn-24r: feat: add login' --reviewers myproject-security"
        );
    }

    /// The command edict prints must create a review the gate can actually find,
    /// whatever the caller passes as a title.
    #[test]
    fn seal_create_titles_are_always_gate_visible() {
        for title in ["feat: add login", "Work from bn-24r", "", "café ☕"] {
            let cmd = seal_create_cmd(
                "frost-castle",
                "crimson-storm",
                "bn-24r",
                title,
                "myproject-security",
            );
            assert!(
                cmd.contains("--title 'bn-24r: "),
                "title {title:?} produced a command the review gate cannot match: {cmd}"
            );
        }
    }

    #[test]
    fn seal_request_basic() {
        let cmd = seal_request_cmd(
            "frost-castle",
            "cr-123",
            "myproject-security",
            "crimson-storm",
        );
        assert_eq!(
            cmd,
            "maw exec frost-castle -- seal reviews request cr-123 --reviewers myproject-security --agent crimson-storm"
        );
    }

    #[test]
    fn seal_show_basic() {
        let cmd = seal_show_cmd("frost-castle", "cr-123");
        assert_eq!(cmd, "maw exec frost-castle -- seal review cr-123");
    }

    #[test]
    fn seal_retarget_basic() {
        let cmd = seal_retarget_cmd("frost-castle", "cr-123", "crimson-storm");
        assert_eq!(
            cmd,
            "maw exec frost-castle -- seal reviews retarget cr-123 --agent crimson-storm"
        );
    }

    // --- Deterministic output tests ---

    #[test]
    fn command_builders_are_deterministic() {
        // Same inputs always produce same output
        let cmd1 = rite_send_cmd("crimson-storm", "proj", "msg", "label");
        let cmd2 = rite_send_cmd("crimson-storm", "proj", "msg", "label");
        assert_eq!(cmd1, cmd2);
    }

    // --- Injection resistance tests ---

    #[test]
    fn escape_prevents_command_injection() {
        // Malicious input with embedded quotes gets properly escaped
        let malicious = "done'; rm -rf /; echo '";
        let escaped = shell_escape(malicious);
        // The escaped value starts and ends with single quotes
        assert!(escaped.starts_with('\''));
        assert!(escaped.ends_with('\''));
        // Embedded single quotes are broken out with \'
        assert!(escaped.contains("\\'"));
        // When used in a command, the entire escaped value appears as one arg
        let cmd = bn_comment_cmd("bd-abc", malicious);
        assert!(cmd.contains(&escaped));
        // Roundtrip: the escaped form should decode back to the original
        // (verified by the start/end quotes and \' escaping pattern)
    }

    #[test]
    fn escape_prevents_variable_expansion() {
        let msg = "Status: $HOME/.secret";
        let escaped = shell_escape(msg);
        assert_eq!(escaped, "'Status: $HOME/.secret'");
    }
}
