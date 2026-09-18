use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::subprocess::run_command;

/// Detected runtime context for hooks.
struct HookContext {
    /// If in a maw repo, the path containing .manifold
    maw_root: Option<std::path::PathBuf>,
    /// If in an edict project, the loaded config
    edict_config: Option<Config>,
    /// Agent name from $AGENT or $`RITE_AGENT`
    agent: Option<String>,
}

impl HookContext {
    fn detect() -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();

        let agent = std::env::var("AGENT")
            .or_else(|_| std::env::var("RITE_AGENT"))
            .ok()
            .filter(|a| validate_agent_name(a));

        // Bare layout keeps maw metadata in a top-level `.manifold/`; the root
        // layout keeps it under `.maw/`. Recognize both.
        let maw_root =
            find_ancestor_with(&cwd, ".manifold").or_else(|| find_ancestor_with(&cwd, ".maw"));

        let edict_config = find_edict_config(&cwd).and_then(|p| Config::load(&p).ok());

        Self {
            maw_root,
            edict_config,
            agent,
        }
    }

    fn channel(&self) -> Option<String> {
        self.edict_config
            .as_ref()
            .map(super::super::config::Config::channel)
    }
}

/// Run session-start hook: maw guidance + agent identity + occupy the identity
///
/// `hook_input` is the harness's stdin payload. Claude Code hooks carry the
/// harness `session_id`; with it the identity is occupied as a rite session
/// attachment, which `rite send` and `rite channel` understand, instead of
/// an ownerless `agent://` claim that `rite channel` would refuse.
pub fn run_session_start(hook_input: Option<&str>) {
    let ctx = HookContext::detect();

    // 1. Maw repo guidance (layout-aware)
    if let Some(ref maw_root) = ctx.maw_root {
        match crate::layout::Layout::detect(maw_root) {
            crate::layout::Layout::Root => println!(
                "This project uses Git + maw for version control. \
                Source files live at the repo root; extra agent workspaces are in \
                .maw/workspaces/<workspace>. Run bn, seal, cargo, etc. directly at the \
                root — no prefix needed. Use `maw exec <workspace> -- <command>` only to \
                run commands inside another workspace. \
                Run `maw --help` for more info."
            ),
            crate::layout::Layout::Bare => println!(
                "This project uses Git + maw for version control. \
                Source files live in workspaces under ws/, not at the project root. \
                Use `maw exec <workspace> -- <command>` to run commands. \
                Run `maw --help` for more info."
            ),
        }
    }

    // 2. Agent identity + project channel (if edict project and agent set)
    if let Some(ref agent) = ctx.agent
        && let Some(ref config) = ctx.edict_config
    {
        println!("Agent ID for use with rite/seal/bn: {agent}");
        println!("Project channel: {}", config.channel());
    }

    // 3. Occupy the identity (if agent set)
    if let Some(ref agent) = ctx.agent {
        match session_id_from(hook_input) {
            Some(session) => {
                attach_session(agent, &session);
            }
            None => stake_claim(agent),
        }
    }
}

/// Run post-tool-call hook: check rite inbox + refresh claim
///
/// # Errors
///
/// Returns `Err` if checking the rite inbox fails (e.g. emitting hook output).
pub fn run_post_tool_call(hook_input: Option<&str>) -> Result<()> {
    let ctx = HookContext::detect();

    let Some(ref agent) = ctx.agent else {
        return Ok(());
    };

    // 1. Check rite inbox
    check_rite_inbox(&ctx, agent, hook_input)?;

    // 2. Keep the identity occupied if its claim is expiring
    match session_id_from(hook_input) {
        Some(session) => renew_attachment_if_needed(agent, &session),
        None => refresh_claim_if_needed(agent),
    }

    Ok(())
}

/// Run session-end hook: detach the session, release the claim, clear status
pub fn run_session_end(hook_input: Option<&str>) {
    let agent = std::env::var("AGENT")
        .or_else(|_| std::env::var("RITE_AGENT"))
        .ok()
        .filter(|a| validate_agent_name(a));

    let Some(agent) = agent else {
        return;
    };

    // The attachment this session made, if any. A session that was never
    // attached, or whose attachment a `rite channel` took over, is a no-op.
    if let Some(session) = session_id_from(hook_input) {
        let _ = run_command(
            "rite",
            &[
                "sessions",
                "detach",
                "--agent",
                &agent,
                "--session",
                &session,
                "-q",
            ],
            None,
        );
    }

    let claim_uri = format!("agent://{agent}");
    let _ = run_command(
        "rite",
        &["claims", "release", "--agent", &agent, &claim_uri, "-q"],
        None,
    );
    let _ = run_command(
        "rite",
        &["statuses", "clear", "--agent", &agent, "-q"],
        None,
    );
}

// --- Internal helpers ---

/// The harness session id from a hook's stdin payload. Claude Code sends a
/// JSON object with `session_id` on every hook event; anything else, or
/// nothing at all, means the harness did not say.
fn session_id_from(hook_input: Option<&str>) -> Option<String> {
    let input = hook_input?;
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let id = value.get("session_id")?.as_str()?.trim();
    if id.is_empty()
        || id.len() > 256
        || id.starts_with('-')
        || !id.chars().all(|c| c.is_ascii_graphic())
    {
        return None;
    }
    Some(id.to_string())
}

/// Why `rite sessions attach` did not attach.
#[derive(Debug, PartialEq, Eq)]
enum AttachRefusal {
    /// This same session is already attached: a compaction re-run. Fine.
    ThisSession,
    /// This agent is attached to another session, such as a `rite channel`
    /// that got there first, or the previous session of a `/clear`. The
    /// identity is occupied; this session is not the occupant.
    OtherSession,
    /// The `rite` binary is not on PATH. The old ownerless stake was silent
    /// about this too.
    NoRite,
    /// Anything else: another agent holds the identity, an ownerless claim
    /// from a responder, a store problem.
    Other(String),
}

/// Classify an attach failure from its message. Both "already attached"
/// messages come from rite's `sessions attach`; the session's own reads
/// `Session <id> is already attached`, the other `<agent> is already attached
/// to <harness> session <other>`.
fn classify_attach_error(session: &str, why: &str) -> AttachRefusal {
    if why.contains("running rite") {
        AttachRefusal::NoRite
    } else if why.contains(&format!("Session {session} is already attached")) {
        AttachRefusal::ThisSession
    } else if why.contains("is already attached to") {
        AttachRefusal::OtherSession
    } else {
        AttachRefusal::Other(why.to_string())
    }
}

/// Occupy `agent://<agent>` as an attachment of the harness session, kind
/// `pull`: the model reads the bus itself, nothing pushes into it. A
/// `rite channel` in the same session takes the attachment over and keeps
/// the identity from there.
///
/// The claim lasts ten minutes and is renewed by tool activity, so a
/// harness that dies lets the identity lapse the way the ownerless claim
/// did. Returns whether this session holds the identity afterwards.
fn attach_session(agent: &str, session: &str) -> bool {
    let result = run_command(
        "rite",
        &[
            "sessions",
            "attach",
            "--agent",
            agent,
            "--harness",
            "claude",
            "--session",
            session,
            "--kind",
            "pull",
            "--ttl",
            "600",
            "-q",
        ],
        None,
    );
    match result {
        Ok(_) => true,
        Err(e) => match classify_attach_error(session, &e.to_string()) {
            AttachRefusal::ThisSession => true,
            AttachRefusal::NoRite => false,
            AttachRefusal::OtherSession => {
                println!(
                    "agent://{agent} is occupied by another session of this agent; this session is not the occupant."
                );
                false
            }
            AttachRefusal::Other(why) => {
                eprintln!("edict: could not occupy agent://{agent} for this session: {why}");
                false
            }
        },
    }
}

/// Who holds this agent's identity, read from a `rite sessions list
/// --format json` document.
#[derive(Debug, PartialEq, Eq)]
enum Occupant {
    /// This session, by the attachment with this id.
    ThisSession(String),
    /// Another session of this agent: a `rite channel` that took over, or a
    /// session that has not ended yet. Nothing for this session to do.
    OtherSession,
    /// Nobody.
    Nobody,
}

fn occupant_in(list_json: &str, session: &str) -> Occupant {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(list_json) else {
        return Occupant::Nobody;
    };
    let Some(sessions) = data["sessions"].as_array() else {
        return Occupant::Nobody;
    };
    let live = sessions
        .iter()
        .filter(|s| s["attached"].as_bool() == Some(true));
    let mut other = false;
    for s in live {
        if s["session"].as_str() == Some(session) {
            if let Some(id) = s["attachment_id"].as_str() {
                return Occupant::ThisSession(id.to_string());
            }
        } else {
            other = true;
        }
    }
    if other {
        Occupant::OtherSession
    } else {
        Occupant::Nobody
    }
}

/// Who holds this agent's identity now.
fn occupant_of(agent: &str, session: &str) -> Occupant {
    run_command(
        "rite",
        &["sessions", "list", "--agent", agent, "--format", "json"],
        None,
    )
    .map_or(Occupant::Nobody, |output| occupant_in(&output, session))
}

/// Whether, in a `rite claims list --all --format json` document, the claim
/// on `claim_uri` held by `principal` is active and has under `threshold`
/// seconds left. A lapsed claim has a negative remainder and counts. A claim
/// held by any other principal, such as the attachment a `rite channel` took
/// over with, does not.
fn claim_needs_renewal(
    claims_json: &str,
    principal: &str,
    claim_uri: &str,
    threshold: i64,
) -> bool {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(claims_json) else {
        return false;
    };
    data["claims"].as_array().is_some_and(|claims| {
        claims.iter().any(|claim| {
            claim["active"].as_bool() == Some(true)
                && claim["agent"].as_str() == Some(principal)
                && claim["patterns"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|x| x.as_str() == Some(claim_uri)))
                && claim["expires_in_secs"]
                    .as_i64()
                    .is_some_and(|left| left < threshold)
        })
    })
}

/// Keep this session's attachment holding the identity. A session that has
/// no live attachment while nobody holds the identity (its start-time attach
/// was refused by a claim that has since lapsed, or a `/clear` started this
/// session before the previous one's detach ran) attaches now; attach is
/// idempotent for this session, so this is cheap. When another session of
/// this agent holds it, above all a `rite channel` that took this session's
/// attachment over, there is nothing to do and nothing to say. An
/// attachment whose claim is about to lapse, or has lapsed, is renewed.
fn renew_attachment_if_needed(agent: &str, session: &str) {
    let attachment = match occupant_of(agent, session) {
        Occupant::ThisSession(id) => id,
        Occupant::OtherSession => return,
        Occupant::Nobody => {
            attach_session(agent, session);
            return;
        }
    };
    let principal = format!("session:{attachment}");
    let claim_uri = format!("agent://{agent}");

    let Ok(output) = run_command(
        "rite",
        &[
            "claims", "list", "--all", "--agent", agent, "--format", "json",
        ],
        None,
    ) else {
        return;
    };
    if claim_needs_renewal(&output, &principal, &claim_uri, 120) {
        let _ = run_command(
            "rite",
            &[
                "sessions",
                "renew",
                "--agent",
                agent,
                "--attachment",
                &attachment,
                "--ttl",
                "600",
                "-q",
            ],
            None,
        );
    }
}

/// The ownerless claim, for a harness whose hooks carry no session id.
fn stake_claim(agent: &str) {
    let claim_uri = format!("agent://{agent}");
    let _ = run_command(
        "rite",
        &[
            "claims", "stake", "--agent", agent, &claim_uri, "--ttl", "600", "-q",
        ],
        None,
    );
}

fn refresh_claim_if_needed(agent: &str) {
    let claim_uri = format!("agent://{agent}");
    let refresh_threshold = 120;

    let list_output = run_command(
        "rite",
        &[
            "claims", "list", "--mine", "--agent", agent, "--format", "json",
        ],
        None,
    )
    .ok();

    if let Some(output) = list_output
        && let Ok(data) = serde_json::from_str::<serde_json::Value>(&output)
        && let Some(claims) = data["claims"].as_array()
    {
        for claim in claims {
            if let Some(patterns) = claim["patterns"].as_array()
                && patterns.iter().any(|p| p.as_str() == Some(&claim_uri))
                && let Some(expires_in) = claim["expires_in_secs"].as_i64()
                && expires_in < refresh_threshold
            {
                let _ = run_command(
                    "rite",
                    &[
                        "claims", "refresh", "--agent", agent, &claim_uri, "--ttl", "600", "-q",
                    ],
                    None,
                );
            }
        }
    }
}

fn check_rite_inbox(ctx: &HookContext, agent: &str, _hook_input: Option<&str>) -> Result<()> {
    let Some(channel) = ctx.channel() else {
        return Ok(()); // No edict project, skip inbox check
    };

    let agent_flag = format!("--agent={agent}");

    // Check unread count
    let count_output = run_command(
        "rite",
        &[
            "inbox",
            &agent_flag,
            "--count-only",
            "--mentions",
            "--channels",
            &channel,
        ],
        None,
    )
    .ok();

    let count: u32 = count_output
        .as_ref()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    if count == 0 {
        return Ok(());
    }

    // Fetch messages as JSON
    let inbox_json = run_command(
        "rite",
        &[
            "inbox",
            &agent_flag,
            "--mentions",
            "--channels",
            &channel,
            "--limit-per-channel",
            "5",
            "--format",
            "json",
        ],
        None,
    )
    .unwrap_or_default();

    let messages = parse_inbox_previews(&inbox_json, Some(agent));

    let mark_read_cmd =
        format!("rite inbox --agent {agent} --mentions --channels {channel} --mark-read");

    let context = format!(
        "STOP: You have {count} unread rite message(s) in #{channel}. Check if any need a response:\n{messages}\n\nTo read and respond: `{mark_read_cmd}`"
    );

    let hook_output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context
        }
    });

    println!("{}", serde_json::to_string(&hook_output)?);

    Ok(())
}

/// Walk up from `start` looking for a directory containing `marker`.
fn find_ancestor_with(start: &Path, marker: &str) -> Option<std::path::PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(marker).exists() {
            return Some(dir);
        }
        // Also check ws/default/ (bare repo layout)
        if dir.join("ws/default").join(marker).exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk up from `start` looking for an edict/botbox config file.
/// Returns the config file path if found.
fn find_edict_config(start: &Path) -> Option<std::path::PathBuf> {
    // Current name first, then legacy names in order of recency
    const CONFIG_NAMES: &[&str] = &[".edict.toml", ".botbox.toml", ".botbox.json"];
    let mut dir = start.to_path_buf();
    loop {
        for name in CONFIG_NAMES {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        // Also check ws/default/
        let ws_default = dir.join("ws/default");
        if ws_default.exists() {
            for name in CONFIG_NAMES {
                let p = ws_default.join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Validates an agent name against `[a-z0-9][a-z0-9-/]*`.
fn validate_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'/')
        && !name.starts_with('-')
        && !name.starts_with('/')
}

fn parse_inbox_previews(inbox_json: &str, agent: Option<&str>) -> String {
    let data: serde_json::Value = match serde_json::from_str(inbox_json) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };

    let mut previews = Vec::new();

    let messages: Vec<&serde_json::Map<String, serde_json::Value>> =
        data["mentions"].as_array().map_or_else(
            || {
                data["messages"].as_array().map_or_else(Vec::new, |arr| {
                    arr.iter().filter_map(|m| m.as_object()).collect()
                })
            },
            |arr| {
                arr.iter()
                    .filter_map(|m| m["message"].as_object())
                    .collect()
            },
        );

    for msg in messages {
        let sender = msg
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let body = msg.get("body").and_then(|v| v.as_str()).unwrap_or("");

        let tag = agent.map_or("", |a| {
            if body.contains(&format!("@{a}")) {
                "[MENTIONS YOU] "
            } else {
                ""
            }
        });

        let mut preview = format!("{tag}{sender}: {body}");
        if preview.len() > 100 {
            preview.truncate(97);
            preview.push_str("...");
        }

        previews.push(format!("  - {preview}"));
    }

    previews.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_ancestor_with_direct() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".manifold")).unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_ancestor_with_ws_default() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("ws/default/.manifold")).unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert_eq!(result, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn session_id_comes_from_the_hook_payload() {
        assert_eq!(
            session_id_from(Some(
                r#"{"session_id":"abc-123","hook_event_name":"SessionStart"}"#
            )),
            Some("abc-123".to_string())
        );
        assert_eq!(
            session_id_from(Some(r#"{"session_id":"  abc  "}"#)),
            Some("abc".to_string())
        );
    }

    #[test]
    fn a_session_id_that_looks_like_a_flag_is_rejected() {
        assert_eq!(session_id_from(Some(r#"{"session_id":"--all"}"#)), None);
        assert_eq!(session_id_from(Some(r#"{"session_id":"-x"}"#)), None);
    }

    #[test]
    fn attach_refusals_are_told_apart() {
        assert_eq!(
            classify_attach_error(
                "s1",
                "rite failed: Error: Session s1 is already attached to a as attachment 01X. Detach it first"
            ),
            AttachRefusal::ThisSession
        );
        assert_eq!(
            classify_attach_error(
                "s1",
                "rite failed: Error: a is already attached to claude session mcp:9 as attachment 01Y. Pass --replace"
            ),
            AttachRefusal::OtherSession
        );
        assert_eq!(
            classify_attach_error("s1", "running rite"),
            AttachRefusal::NoRite
        );
        assert!(matches!(
            classify_attach_error("s1", "rite failed: Error: agent://a is held by b until ..."),
            AttachRefusal::Other(_)
        ));
    }

    const LIST: &str = r#"{"sessions":[
        {"attachment_id":"01A","agent":"a","harness":"claude","session":"old","kind":"pull","state":"detached","attached":false},
        {"attachment_id":"01B","agent":"a","harness":"claude","session":"s1","kind":"pull","state":"attached","attached":true}
    ]}"#;

    #[test]
    fn the_occupant_is_told_from_the_session_list() {
        assert_eq!(
            occupant_in(LIST, "s1"),
            Occupant::ThisSession("01B".to_string())
        );
        // A detached record for this session and a live one for another:
        // the other holds it, and this session stays quiet.
        assert_eq!(occupant_in(LIST, "old"), Occupant::OtherSession);
        assert_eq!(occupant_in(LIST, "nope"), Occupant::OtherSession);
        assert_eq!(occupant_in(r#"{"sessions":[]}"#, "s1"), Occupant::Nobody);
        assert_eq!(
            occupant_in(
                r#"{"sessions":[{"attachment_id":"01A","session":"old","attached":false}]}"#,
                "s1"
            ),
            Occupant::Nobody
        );
        assert_eq!(occupant_in("not json", "s1"), Occupant::Nobody);
    }

    fn claims(principal: &str, left: i64, active: bool) -> String {
        format!(
            r#"{{"claims":[{{"agent":"{principal}","patterns":["agent://a"],"active":{active},"expires_in_secs":{left}}}]}}"#
        )
    }

    #[test]
    fn a_claim_near_or_past_expiry_is_renewed_only_when_it_is_ours() {
        assert!(claim_needs_renewal(
            &claims("session:01B", 99, true),
            "session:01B",
            "agent://a",
            120
        ));
        assert!(claim_needs_renewal(
            &claims("session:01B", -5, true),
            "session:01B",
            "agent://a",
            120
        ));
        assert!(!claim_needs_renewal(
            &claims("session:01B", 599, true),
            "session:01B",
            "agent://a",
            120
        ));
        assert!(!claim_needs_renewal(
            &claims("session:01B", 99, false),
            "session:01B",
            "agent://a",
            120
        ));
        // A rite channel took the identity over: its claim is not ours to renew.
        assert!(!claim_needs_renewal(
            &claims("session:01C", 99, true),
            "session:01B",
            "agent://a",
            120
        ));
        assert!(!claim_needs_renewal(
            &claims("session:01B", 99, true),
            "session:01B",
            "agent://b",
            120
        ));
        assert!(!claim_needs_renewal(
            "nope",
            "session:01B",
            "agent://a",
            120
        ));
    }

    #[test]
    fn session_id_is_absent_when_the_harness_did_not_say() {
        assert_eq!(session_id_from(None), None);
        assert_eq!(session_id_from(Some("")), None);
        assert_eq!(session_id_from(Some("not json")), None);
        assert_eq!(
            session_id_from(Some(r#"{"hook_event_name":"SessionStart"}"#)),
            None
        );
        assert_eq!(session_id_from(Some(r#"{"session_id":""}"#)), None);
        assert_eq!(session_id_from(Some(r#"{"session_id":"has space"}"#)), None);
        assert_eq!(session_id_from(Some(r#"{"session_id":42}"#)), None);
    }

    #[test]
    fn find_ancestor_with_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_ancestor_with(tmp.path(), ".manifold");
        assert!(result.is_none());
    }

    #[test]
    fn find_edict_config_edict_toml_preferred() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".edict.toml"), "").unwrap();
        fs::write(tmp.path().join(".botbox.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(result, Some(tmp.path().join(".edict.toml")));
    }

    #[test]
    fn find_edict_config_legacy_toml_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".botbox.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(result, Some(tmp.path().join(".botbox.toml")));
    }

    #[test]
    fn find_edict_config_ws_default() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("ws/default")).unwrap();
        fs::write(tmp.path().join("ws/default/.edict.toml"), "").unwrap();
        let result = find_edict_config(tmp.path());
        assert_eq!(result, Some(tmp.path().join("ws/default/.edict.toml")));
    }

    #[test]
    fn find_edict_config_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_edict_config(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn parse_inbox_previews_empty() {
        let json = r#"{"mentions":[]}"#;
        let result = parse_inbox_previews(json, None);
        assert_eq!(result, "");
    }

    #[test]
    fn parse_inbox_previews_with_messages() {
        let json = r#"{
            "mentions": [
                {
                    "message": {
                        "agent": "alice",
                        "body": "Hey @bob, check this out"
                    }
                }
            ]
        }"#;
        let result = parse_inbox_previews(json, Some("bob"));
        assert!(result.contains("[MENTIONS YOU]"));
        assert!(result.contains("alice"));
    }

    #[test]
    fn parse_inbox_previews_truncation() {
        let long_body = "a".repeat(200);
        let json = format!(
            r#"{{"mentions": [{{"message": {{"agent": "sender", "body": "{long_body}"}}}}]}}"#
        );
        let result = parse_inbox_previews(&json, None);
        assert!(result.len() < 150);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn validate_agent_name_accepts_valid() {
        assert!(validate_agent_name("botbox-dev"));
        assert!(validate_agent_name("botbox-dev/worker-1"));
        assert!(validate_agent_name("a"));
        assert!(validate_agent_name("agent123"));
    }

    #[test]
    fn validate_agent_name_rejects_invalid() {
        assert!(!validate_agent_name(""));
        assert!(!validate_agent_name("-starts-dash"));
        assert!(!validate_agent_name("/starts-slash"));
        assert!(!validate_agent_name("Has Uppercase"));
        assert!(!validate_agent_name("has space"));
        assert!(!validate_agent_name("$(inject)"));
        assert!(!validate_agent_name("--help"));
    }
}
