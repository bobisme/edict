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
        .map(|c| c.is_ascii_alphanumeric())
        .unwrap_or(false)
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
            ValidationError::Empty(field) => write!(f, "{field} cannot be empty"),
            ValidationError::TooLong {
                field, max, actual, ..
            } => {
                write!(f, "{field} too long ({actual} chars, max {max})")
            }
            ValidationError::InvalidFormat {
                field,
                value,
                expected,
            } => {
                write!(f, "invalid {field} '{value}', expected {expected}")
            }
            ValidationError::UnsafeChars { field, value } => {
                write!(f, "{field} '{value}' contains shell metacharacters")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate a review ID (e.g., `cr-2rnh`).
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

// --- Command builders ---
// These produce shell-safe command strings. All dynamic values are validated
// or escaped before inclusion. Structural identifiers pass through safe_ident()
// for defense-in-depth against unvalidated callers.

/// Build: `bus claims stake --agent <agent> "bone://<project>/<id>" -m "<memo>"`
pub fn claims_stake_cmd(agent: &str, uri: &str, memo: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    let mut cmd = String::new();
    write!(
        cmd,
        "bus claims stake --agent {} {}",
        agent_safe,
        shell_escape(uri)
    )
    .unwrap();
    if !memo.is_empty() {
        write!(cmd, " -m {}", shell_escape(memo)).unwrap();
    }
    cmd
}

/// Build: `bus claims release --agent <agent> "<uri>"`
#[allow(dead_code)]
pub fn claims_release_cmd(agent: &str, uri: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!(
        "bus claims release --agent {} {}",
        agent_safe,
        shell_escape(uri)
    )
}

/// Build: `bus claims release --agent <agent> --all`
pub fn claims_release_all_cmd(agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!("bus claims release --agent {} --all", agent_safe)
}

/// Build: `bus send --agent <agent> <project> '<message>' -L <label>`
pub fn bus_send_cmd(agent: &str, project: &str, message: &str, label: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

    // Validate project name before use
    if let Err(_) = validate_identifier("project", project) {
        // If validation fails, force escaping instead of raw interpolation
        let mut cmd = String::new();
        write!(
            cmd,
            "bus send --agent {} {} {}",
            agent_safe,
            shell_escape(project),
            shell_escape(message)
        )
        .unwrap();
        if !label.is_empty() {
            write!(cmd, " -L {}", shell_escape(label)).unwrap();
        }
        return cmd;
    }

    let mut cmd = String::new();
    write!(
        cmd,
        "bus send --agent {} {} {}",
        agent_safe,
        safe_ident(project),
        shell_escape(message)
    )
    .unwrap();
    if !label.is_empty() {
        // Apply same validate+escape fallback as project parameter
        if validate_identifier("label", label).is_ok() {
            write!(cmd, " -L {}", safe_ident(label)).unwrap();
        } else {
            write!(cmd, " -L {}", shell_escape(label)).unwrap();
        }
    }
    cmd
}

/// Build: `maw exec default -- bn do <id>`
#[allow(dead_code)]
pub fn bn_do_cmd(bone_id: &str) -> String {
    // Validate bone_id before use - escape if validation fails
    let bone_id_safe = if validate_bone_id(bone_id).is_ok() {
        safe_ident(bone_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(bone_id))
    };

    format!("maw exec default -- bn do {}", bone_id_safe)
}

/// Build: `maw exec default -- bn bone comment add <id> '<message>'`
#[allow(dead_code)]
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
pub fn bn_done_cmd(bone_id: &str, reason: &str) -> String {
    // Validate bone_id before use
    let bone_id_safe = if validate_bone_id(bone_id).is_ok() {
        safe_ident(bone_id)
    } else {
        std::borrow::Cow::Owned(shell_escape(bone_id))
    };

    let mut cmd = format!("maw exec default -- bn done {}", bone_id_safe);
    if !reason.is_empty() {
        write!(cmd, " --reason {}", shell_escape(reason)).unwrap();
    }
    cmd
}

/// Build: `maw ws create --random`
pub fn ws_create_cmd() -> String {
    "maw ws create --random".to_string()
}

/// Build: `maw ws merge <ws> --destroy --message <msg>`
///
/// `message` is required — maw enforces explicit commit messages.
/// Use conventional commit prefix: `feat:`, `fix:`, `chore:`, etc.
pub fn ws_merge_cmd(workspace: &str, message: &str) -> String {
    // Validate workspace name before use
    let workspace_safe = if validate_workspace_name(workspace).is_ok() {
        safe_ident(workspace)
    } else {
        std::borrow::Cow::Owned(shell_escape(workspace))
    };

    format!(
        "maw ws merge {} --destroy --message {}",
        workspace_safe,
        shell_escape(message)
    )
}

/// Build: `maw exec <ws> -- seal reviews create --agent <agent> --title '<title>' --reviewers <reviewers>`
pub fn seal_create_cmd(workspace: &str, agent: &str, title: &str, reviewers: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);

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
        shell_escape(title),
        reviewers_safe
    )
}

/// Build: `maw exec <ws> -- seal reviews request <id> --reviewers <reviewers> --agent <agent>`
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
        "maw exec {} -- seal reviews request {} --reviewers {} --agent {}",
        workspace_safe, review_id_safe, reviewers_safe, agent_safe
    )
}

/// Build: `maw exec <ws> -- seal review <id>`
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

    format!(
        "maw exec {} -- seal review {}",
        workspace_safe, review_id_safe
    )
}

/// Build: `bus statuses clear --agent <agent>`
pub fn bus_statuses_clear_cmd(agent: &str) -> String {
    validate_identifier("agent", agent).expect("invalid agent name");
    let agent_safe = safe_ident(agent);
    format!("bus statuses clear --agent {}", agent_safe)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "bus claims stake --agent crimson-storm 'bone://myproject/bd-abc' -m 'bd-abc'"
        );
    }

    #[test]
    fn claims_stake_no_memo() {
        let cmd = claims_stake_cmd("crimson-storm", "bone://myproject/bd-abc", "");
        assert_eq!(
            cmd,
            "bus claims stake --agent crimson-storm 'bone://myproject/bd-abc'"
        );
    }

    #[test]
    fn claims_release_basic() {
        let cmd = claims_release_cmd("crimson-storm", "bone://myproject/bd-abc");
        assert_eq!(
            cmd,
            "bus claims release --agent crimson-storm 'bone://myproject/bd-abc'"
        );
    }

    #[test]
    fn claims_release_all() {
        let cmd = claims_release_all_cmd("crimson-storm");
        assert_eq!(cmd, "bus claims release --agent crimson-storm --all");
    }

    #[test]
    fn bus_send_basic() {
        let cmd = bus_send_cmd(
            "crimson-storm",
            "myproject",
            "Task claimed: bd-abc",
            "task-claim",
        );
        assert_eq!(
            cmd,
            "bus send --agent crimson-storm myproject 'Task claimed: bd-abc' -L task-claim"
        );
    }

    #[test]
    fn bus_send_with_quotes_in_message() {
        let cmd = bus_send_cmd("crimson-storm", "myproject", "it's done", "task-done");
        assert_eq!(
            cmd,
            "bus send --agent crimson-storm myproject 'it'\\''s done' -L task-done"
        );
    }

    #[test]
    fn bus_send_no_label() {
        let cmd = bus_send_cmd("crimson-storm", "myproject", "hello", "");
        assert_eq!(cmd, "bus send --agent crimson-storm myproject 'hello'");
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
    fn ws_merge_with_message() {
        let cmd = ws_merge_cmd("frost-castle", "feat: add login flow");
        assert_eq!(
            cmd,
            "maw ws merge frost-castle --destroy --message 'feat: add login flow'"
        );
    }

    #[test]
    fn seal_create_with_escaping() {
        let cmd = seal_create_cmd(
            "frost-castle",
            "crimson-storm",
            "feat: add login",
            "myproject-security",
        );
        assert_eq!(
            cmd,
            "maw exec frost-castle -- seal reviews create --agent crimson-storm --title 'feat: add login' --reviewers myproject-security"
        );
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

    // --- Deterministic output tests ---

    #[test]
    fn command_builders_are_deterministic() {
        // Same inputs always produce same output
        let cmd1 = bus_send_cmd("crimson-storm", "proj", "msg", "label");
        let cmd2 = bus_send_cmd("crimson-storm", "proj", "msg", "label");
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
