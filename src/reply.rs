//! Reply anchors for rite threading (rite >= 0.33).
//!
//! A rite message can anchor to a parent: `rite send ... --reply-to <ULID>`.
//! Threading only holds if the agent is told its anchor on EVERY turn. An agent
//! told once at spawn drifts within a few turns; an agent told the current
//! anchor every turn does not. Every loop prompt therefore appends
//! [`anchor_section`] on each iteration, not only on the first one.
//!
//! The anchor a hook-spawned agent must use is the message that woke it:
//! `RITE_MESSAGE_ID`, or the last id in `RITE_BATCH_MESSAGE_IDS` when a
//! lease-enabled hook handed it a batch (that list is chronological, and the
//! triggering message is last).

/// Env vars forwarded to hook-spawned agents, as `vessel spawn --env-inherit`
/// takes them: the whole `RITE_` namespace plus the few vars outside it.
///
/// One namespace covers every var rite sets at spawn — `RITE_CHANNEL`,
/// `RITE_MESSAGE_ID`, `RITE_HOOK_ID`, and the lease-only `RITE_BATCH_COUNT`,
/// `RITE_BATCH_MESSAGE_IDS`, `RITE_LEASE_PATTERN` — so a var rite adds later
/// reaches the agent without an edict release. It also carries `RITE_DATA_DIR`,
/// which keeps an isolated test store isolated across a spawn.
///
/// `RITE_AGENT` comes along too. rite sets it to the hook's `claim_owner` when
/// there is one, which for every edict hook is the spawned agent itself, not
/// the sender. The loops override it from `--agent` regardless.
pub const HOOK_ENV_INHERIT: &str = "RITE_*,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";

/// The pre-wildcard form, for a vessel too old to expand `RITE_*`.
///
/// Such a vessel treats `RITE_*` as a literal variable name, inherits nothing,
/// and leaves the agent without `RITE_CHANNEL`.
pub const HOOK_ENV_INHERIT_EXPLICIT: &str = "RITE_CHANNEL,RITE_MESSAGE_ID,RITE_AGENT,RITE_HOOK_ID,RITE_BATCH_COUNT,RITE_BATCH_MESSAGE_IDS,RITE_LEASE_PATTERN,SSH_AUTH_SOCK,OTEL_EXPORTER_OTLP_ENDPOINT,TRACEPARENT";

/// The `--env-inherit` value to register, matched to the installed vessel.
#[must_use]
pub fn hook_env_inherit() -> &'static str {
    if crate::subprocess::vessel_supports_env_prefix() {
        HOOK_ENV_INHERIT
    } else {
        HOOK_ENV_INHERIT_EXPLICIT
    }
}

/// Seconds a loop blocks on `rite wait --reply-to` before it escalates.
///
/// Short enough to leave room inside an agent timeout (900s for dev and worker
/// loops), long enough for a spawned reviewer to start and answer.
pub const DEFAULT_WAIT_TIMEOUT: u64 = 300;

/// Report whether `s` looks like a ULID (26 Crockford base32 characters).
///
/// rite rejects a non-ULID anchor with exit code 2, so the loops filter obvious
/// junk out of the environment before they put it in a prompt.
#[must_use]
pub fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.bytes().all(|b| match b {
            b'0'..=b'9' => true,
            b'A'..=b'Z' => !matches!(b, b'I' | b'L' | b'O' | b'U'),
            _ => false,
        })
}

/// Resolve the reply anchor for this turn from the hook environment.
///
/// Prefers the last id of `RITE_BATCH_MESSAGE_IDS` (chronological, triggering
/// message last), then `RITE_MESSAGE_ID`. Returns `None` when neither holds a
/// ULID.
#[must_use]
pub fn anchor_from_env() -> Option<String> {
    anchor_from_parts(
        std::env::var("RITE_BATCH_MESSAGE_IDS").ok().as_deref(),
        std::env::var("RITE_MESSAGE_ID").ok().as_deref(),
    )
}

/// Pure form of [`anchor_from_env`], for testing.
#[must_use]
pub fn anchor_from_parts(batch_ids: Option<&str>, message_id: Option<&str>) -> Option<String> {
    let from_batch = batch_ids
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .rfind(|id| is_ulid(id))
        .map(ToString::to_string);

    from_batch.or_else(|| {
        message_id
            .map(str::trim)
            .filter(|id| is_ulid(id))
            .map(ToString::to_string)
    })
}

/// Build the per-turn REPLY ANCHOR section for a loop prompt.
///
/// Returns an empty string when there is no anchor, so a manually started loop
/// (no hook, no triggering message) gets no misleading instruction.
#[must_use]
pub fn anchor_section(anchor: Option<&str>, agent: &str, channel: &str) -> String {
    let Some(anchor) = anchor.filter(|a| is_ulid(a)) else {
        return String::new();
    };

    format!(
        r#"
## REPLY ANCHOR (this turn only)

Anchor: {anchor}

- Anchor every message you send about this request to it:
  rite send --agent {agent} {channel} "<message>" --reply-to {anchor}
- Read the whole exchange with: rite history --thread {anchor}
  A thread reported as `complete:false` is a fragment, not the whole conversation.
  Say so instead of treating it as the full context.
- Post a top-level message (no --reply-to) only for an unrelated subject.
- This anchor is valid for this turn only. Later turns carry a different anchor.
  Never reuse an anchor from an earlier turn.
"#
    )
}

/// Build the ASK AND WAIT section for a loop prompt.
///
/// This replaces the post-and-hope pattern: anchor the request, block on the
/// answer, and escalate on timeout instead of asking again. Requester retries
/// were the original source of the message storm.
#[must_use]
pub fn ask_and_wait_section(agent: &str, timeout: u64) -> String {
    format!(
        r#"
## ASK AND WAIT (never post and hope)

When you need an answer before you can continue, capture the message id and
block on the reply:

  id=$(rite send --agent {agent} <channel> "<question> @<target-agent>" -L <label> --format json | jq -r .id)
  rite wait --agent {agent} --reply-to "$id" -t {timeout} --format json

Exit codes:
- 0 — answered. Read `.message.body` from the JSON and act on it.
- 1 — nobody answered inside the timeout. ESCALATE: post ONE message with
  -L task-blocked that names the anchor id, record the id on the bone, and move
  to other work. Do NOT send the request again.
- 2 — the id is not a ULID, or this store never saw it. Fix the id — re-read it
  with `rite history <channel> --from {agent} -n 1 --format json`. Do NOT send
  the request again. Add --allow-missing-parent only when the parent is still
  syncing in from another machine.

Semantics that are easy to get wrong:
- --reply-to narrows the wait. It never widens it. --from, -c and -L only
  subtract candidate answers from it.
- With no -c, every channel counts, so a reply in a DM satisfies the wait.
- A reply that arrived before the wait started still counts. There is no race
  between send and wait.
- Your own reply never satisfies your own wait.
"#
    )
}

/// Which review message the recipe sends: a first request or a re-request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAsk {
    /// First request for a new review.
    New,
    /// Re-request after the author addressed blocking feedback.
    Update,
}

/// Build the review-request recipe: anchor the request, then launch the exact
/// dedicated Daybreak reviewer described in the managed workflow document.
///
/// `indent` is the leading whitespace of the surrounding prompt step, so the
/// recipe reads as part of it. The dev loop and the worker loop share this one
/// text, so the two flows cannot drift apart.
#[must_use]
pub fn review_recipe(ask: ReviewAsk, agent: &str, project: &str, indent: &str) -> String {
    let (message, label) = match ask {
        ReviewAsk::New => (
            "Dedicated security review requested: <review-id> for <id>".to_string(),
            "review-request",
        ),
        ReviewAsk::Update => (
            "Dedicated security re-review requested: <review-id> — addressed feedback".to_string(),
            "review-response",
        ),
    };

    let body = format!(
        r#"ANCHOR the request, then launch the dedicated reviewer — do not post and hope:
  req=$(rite send --agent {agent} {project} "{message}" -L {label} --format json | jq -r .id)
  maw exec default -- bn bone comment add <id> "Review anchor: $req for <review-id>"
  Set `review_id=<review-id>`, `ws=$WS`, `request_anchor=$req`, and `kind={label}`. Then read and
  execute `.agents/edict/security-review.md`'s **Launch contract** exactly. It starts one named,
  labelled Vessel Codex session on `gpt-daybreak-blue-latest`, passes those exact values, waits
  through Agentbus, and has the AUTHOR perform the Rite handoff and terminate that exact session.
  Do not use an @mention or scan for another pending review.
- Agentbus done + a real Seal vote from `{project}-security` on this review is required before
  continuing. Confirm it with `maw exec $WS -- seal review <review-id> --format json`.
  LGTM -> continue to finish in THIS iteration. BLOCKED -> fix the threads now, retarget the
  review to the fixed commits (`maw exec $WS -- seal reviews retarget <review-id> --agent
  {agent}` — `seal reviews request` alone leaves the review's target commit pinned at the old,
  pre-fix anchor), then re-request the same review with a NEW Rite anchor.
- Agentbus unresolved, blocked, unavailable, or timeout: snapshot and terminate the named Vessel
  session, then post one anchored `task-blocked` message and record it on the bone. Release
  `review://{project}/<review-id>` only after teardown succeeds; otherwise retain the claim and
  STOP. Do NOT auto-retry, send an @mention, or substitute another review."#
    );

    body.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_shape_is_checked() {
        assert!(is_ulid("01KZRT64ACRZQRDS79P5ZJ4C3F"));
        assert!(!is_ulid(""));
        assert!(!is_ulid("01KZRT64ACRZQRDS79P5ZJ4C3"), "too short");
        assert!(!is_ulid("01KZRT64ACRZQRDS79P5ZJ4C3FF"), "too long");
        assert!(!is_ulid("01kzrt64acrzqrds79p5zj4c3f"), "lowercase");
        assert!(!is_ulid("01KZRT64ACRZQRDS79P5ZJ4C3I"), "excluded letter I");
        assert!(!is_ulid("01KZRT64ACRZQRDS79P5ZJ4C-F"), "punctuation");
    }

    #[test]
    fn batch_anchor_is_the_last_id() {
        let batch = "01KZRT64ACRZQRDS79P5ZJ4C3F,01KZRT6BDZS1H145FT15TP7RAM";
        assert_eq!(
            anchor_from_parts(Some(batch), Some("01KZRT6NP05XAYX6WJBK4CYXJS")).as_deref(),
            Some("01KZRT6BDZS1H145FT15TP7RAM"),
            "the triggering message is last in the batch"
        );
    }

    #[test]
    fn message_id_is_the_fallback() {
        assert_eq!(
            anchor_from_parts(None, Some("01KZRT6NP05XAYX6WJBK4CYXJS")).as_deref(),
            Some("01KZRT6NP05XAYX6WJBK4CYXJS")
        );
        assert_eq!(
            anchor_from_parts(Some(""), Some("01KZRT6NP05XAYX6WJBK4CYXJS")).as_deref(),
            Some("01KZRT6NP05XAYX6WJBK4CYXJS")
        );
    }

    #[test]
    fn junk_anchors_are_dropped() {
        assert_eq!(anchor_from_parts(Some("nope"), Some("also-nope")), None);
        assert_eq!(anchor_from_parts(None, None), None);
    }

    #[test]
    fn anchor_section_is_empty_without_an_anchor() {
        assert!(anchor_section(None, "edict-dev", "edict").is_empty());
        assert!(anchor_section(Some("not-a-ulid"), "edict-dev", "edict").is_empty());
    }

    #[test]
    fn anchor_section_names_the_anchor_and_thread_read() {
        let section = anchor_section(Some("01KZRT64ACRZQRDS79P5ZJ4C3F"), "edict-dev", "edict");
        assert!(section.contains("--reply-to 01KZRT64ACRZQRDS79P5ZJ4C3F"));
        assert!(section.contains("rite history --thread 01KZRT64ACRZQRDS79P5ZJ4C3F"));
        assert!(section.contains("--agent edict-dev edict"));
    }

    #[test]
    fn ask_and_wait_section_teaches_all_three_exit_codes() {
        let section = ask_and_wait_section("edict-dev", 300);
        assert!(section.contains("-t 300"));
        assert!(section.contains("ESCALATE"));
        assert!(section.contains("Do NOT send the request again."));
    }

    #[test]
    fn review_recipe_indents_every_line_and_covers_each_exit() {
        let recipe = review_recipe(ReviewAsk::New, "edict-dev", "edict", "    ");
        assert!(
            recipe
                .lines()
                .all(|l| l.is_empty() || l.starts_with("    "))
        );
        assert!(recipe.contains("security-review.md"));
        assert!(recipe.contains("gpt-daybreak-blue-latest"));
        assert!(recipe.contains("Agentbus"));
        assert!(recipe.contains("AUTHOR perform the Rite handoff"));
        assert!(recipe.contains("only after teardown succeeds"));
        assert_eq!(
            recipe.matches("Do NOT auto-retry").count(),
            1,
            "a failed direct-review launch must not silently retry"
        );
    }

    /// A blocked review must be retargeted to the fixed commits before it is
    /// re-requested (bn-w912): `seal reviews request` alone leaves the
    /// review's target commit pinned at the pre-fix anchor.
    #[test]
    fn review_recipe_retargets_before_re_requesting_a_blocked_review() {
        let recipe = review_recipe(ReviewAsk::New, "edict-dev", "edict", "");
        let retarget_pos = recipe
            .find("seal reviews retarget <review-id>")
            .expect("recipe must teach retargeting a blocked review");
        let request_pos = recipe
            .find("re-request the same review")
            .expect("recipe must teach re-requesting a blocked review");
        assert!(
            retarget_pos < request_pos,
            "retarget must be taught before re-request"
        );
    }

    #[test]
    fn review_recipe_uses_the_label_of_the_ask() {
        let new = review_recipe(ReviewAsk::New, "edict-dev", "edict", "");
        assert!(new.contains("Dedicated security review requested: <review-id> for <id>"));
        assert!(!new.contains("@edict-security"));
        assert!(new.contains("-L review-request"));

        let update = review_recipe(ReviewAsk::Update, "edict-dev", "edict", "");
        assert!(
            update.contains(
                "Dedicated security re-review requested: <review-id> — addressed feedback"
            )
        );
        assert!(update.contains("-L review-response"));
    }

    #[test]
    fn env_inherit_forms_cover_the_same_rite_namespace() {
        // The wildcard stands in for every RITE_ var the explicit list names,
        // so a vessel that cannot expand it loses nothing.
        assert!(HOOK_ENV_INHERIT.split(',').any(|v| v == "RITE_*"));
        for var in HOOK_ENV_INHERIT_EXPLICIT.split(',') {
            if let Some(rest) = var.strip_prefix("RITE_") {
                assert!(!rest.is_empty(), "RITE_ prefix alone is not a variable");
            } else {
                assert!(
                    HOOK_ENV_INHERIT.split(',').any(|v| v == var),
                    "{var} is outside the RITE_ namespace and must be listed explicitly"
                );
            }
        }
    }

    #[test]
    fn explicit_env_inherit_names_every_var_rite_sets() {
        // Mirrors rite src/cli/hooks.rs: the four always set, plus the three a
        // lease adds.
        for var in [
            "RITE_CHANNEL",
            "RITE_MESSAGE_ID",
            "RITE_AGENT",
            "RITE_HOOK_ID",
            "RITE_BATCH_COUNT",
            "RITE_BATCH_MESSAGE_IDS",
            "RITE_LEASE_PATTERN",
        ] {
            assert!(
                HOOK_ENV_INHERIT_EXPLICIT.split(',').any(|v| v == var),
                "{var} missing from the pre-wildcard list"
            );
        }
    }
}
