//! Whether the message a responder turn answers was addressed to the responder.
//!
//! The router hook fires on every message in the project channel, so the
//! responder also wakes for traffic between other agents — `@probe-codex ping`
//! on #rite, say. When a turn on such a message fails, posting "Could not answer
//! that" into the thread is noise in a conversation the responder was never part
//! of. The failure is still logged and counted; it is only not posted.
//!
//! A message is addressed to the responder when any of these hold:
//!
//! - the channel is a DM between the responder and someone else;
//! - it carries a responder command prefix (`!q`, `!dev`, `q:`, ...);
//! - it @mentions the responder;
//! - it replies to a message the responder wrote;
//! - it is undirected channel traffic: a top-level message that mentions no one.
//!   The responder is the channel's router, and a bare message is how a human
//!   asks it something (the triage route).
//!
//! It is not addressed when it @mentions only other agents, or replies to
//! someone else's message without mentioning the responder.
//!
//! When the answer cannot be known — the message could not be fetched, or its
//! parent is not in the fetched thread — the result is [`Addressing::Unknown`],
//! and the failure is posted. Fail open toward telling the human.

use serde::Deserialize;

use super::responder::{RouteType, route_message};

/// Whether a turn's message was addressed to the responder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    Addressed,
    NotAddressed,
    /// The message could not be classified; treat it as addressed.
    Unknown,
}

impl Addressing {
    /// Whether a turn failure on this message should be posted to the channel.
    #[must_use]
    pub const fn posts_failure(self) -> bool {
        !matches!(self, Self::NotAddressed)
    }

    /// Value for the `addressed` telemetry attribute.
    #[must_use]
    pub const fn as_attr(self) -> &'static str {
        match self {
            Self::Addressed => "true",
            Self::NotAddressed => "false",
            Self::Unknown => "unknown",
        }
    }

    /// Combine the classification of every message in a batch.
    ///
    /// A batch is addressed if any message in it is. It is quiet only when every
    /// message is known to be unaddressed; an empty batch is unknown.
    #[must_use]
    pub fn of_batch(items: impl IntoIterator<Item = Self>) -> Self {
        let mut result = None;
        for item in items {
            result = Some(match (result, item) {
                (_, Self::Addressed) | (Some(Self::Addressed), _) => Self::Addressed,
                (_, Self::Unknown) | (Some(Self::Unknown), _) => Self::Unknown,
                _ => Self::NotAddressed,
            });
        }
        result.unwrap_or(Self::Unknown)
    }
}

/// One message as `rite history --thread <id> --format json` reports it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ThreadMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadResponse {
    #[serde(default)]
    messages: Vec<ThreadMessage>,
}

/// Parse `rite history --thread` JSON output.
#[must_use]
pub fn parse_thread(json: &str) -> Option<Vec<ThreadMessage>> {
    serde_json::from_str::<ThreadResponse>(json)
        .ok()
        .map(|r| r.messages)
}

/// Whether `channel` is the rite DM channel between `agent` and someone else.
///
/// rite names DM channels `_dm_{a}_{b}`, the two agent names sorted.
#[must_use]
pub fn is_dm_with(channel: &str, agent: &str) -> bool {
    let Some(pair) = channel.strip_prefix("_dm_") else {
        return false;
    };
    pair.strip_prefix(agent)
        .is_some_and(|rest| rest.starts_with('_'))
        || pair
            .strip_suffix(agent)
            .is_some_and(|rest| rest.ends_with('_'))
}

/// `@name` tokens in `body`, parsed the way rite parses them.
fn body_mentions(body: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '@' {
            continue;
        }
        let mut name = String::new();
        while let Some(&next) = chars.peek() {
            if next.is_alphanumeric() || next == '_' || next == '-' {
                name.push(next);
                chars.next();
            } else {
                break;
            }
        }
        if !name.is_empty() {
            mentions.push(name);
        }
    }
    mentions
}

/// Classify one message given the author of its parent, if it has one.
///
/// `parent_author` is `None` both for a top-level message and for a reply whose
/// parent could not be found; `message.reply_to` tells the two apart.
#[must_use]
pub fn classify(agent: &str, message: &ThreadMessage, parent_author: Option<&str>) -> Addressing {
    if route_message(&message.body).kind != RouteType::Triage {
        return Addressing::Addressed;
    }

    let mut mentions = message.mentions.clone();
    mentions.extend(body_mentions(&message.body));
    if mentions.iter().any(|m| m.eq_ignore_ascii_case(agent)) {
        return Addressing::Addressed;
    }

    match (message.reply_to.as_deref(), parent_author) {
        (Some(_), Some(author)) if author == agent => Addressing::Addressed,
        // A reply whose parent is missing from the thread: it may answer the
        // responder, so do not stay quiet on a guess.
        (Some(_), None) => Addressing::Unknown,
        (None, _) if mentions.is_empty() => Addressing::Addressed,
        // Someone else's thread, or a top-level message for someone else.
        (Some(_), Some(_)) | (None, _) => Addressing::NotAddressed,
    }
}

/// Classify message `id` from the thread that contains it.
#[must_use]
pub fn classify_in_thread(agent: &str, id: &str, thread: &[ThreadMessage]) -> Addressing {
    let Some(message) = thread.iter().find(|m| m.id == id) else {
        return Addressing::Unknown;
    };
    let parent_author = message
        .reply_to
        .as_deref()
        .and_then(|parent| thread.iter().find(|m| m.id == parent))
        .map(|m| m.agent.as_str());
    classify(agent, message, parent_author)
}

/// Classify the messages a turn answers.
///
/// `fetch_thread` returns the thread containing a message id, or `None` when it
/// cannot be read. A DM channel short-circuits without fetching anything.
pub fn resolve(
    agent: &str,
    channel: &str,
    ids: &[String],
    mut fetch_thread: impl FnMut(&str) -> Option<Vec<ThreadMessage>>,
) -> Addressing {
    if is_dm_with(channel, agent) {
        return Addressing::Addressed;
    }
    Addressing::of_batch(ids.iter().map(|id| {
        fetch_thread(id).map_or(Addressing::Unknown, |thread| {
            classify_in_thread(agent, id, &thread)
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "rite-dev";

    fn msg(id: &str, agent: &str, body: &str, reply_to: Option<&str>) -> ThreadMessage {
        ThreadMessage {
            id: id.to_string(),
            agent: agent.to_string(),
            body: body.to_string(),
            mentions: body_mentions(body),
            reply_to: reply_to.map(ToString::to_string),
        }
    }

    #[test]
    fn a_mention_of_the_responder_is_addressed() {
        let m = msg("A", "bob", "@rite-dev why is sync slow?", None);
        assert_eq!(classify(ME, &m, None), Addressing::Addressed);
    }

    #[test]
    fn a_mention_in_the_body_counts_without_the_mentions_field() {
        let mut m = msg("A", "bob", "hey @rite-dev, a question", None);
        m.mentions.clear();
        assert_eq!(classify(ME, &m, None), Addressing::Addressed);
    }

    #[test]
    fn a_ping_for_another_agent_is_not_addressed() {
        let m = msg(
            "A",
            "probe-claude",
            "@probe-codex ping 1 (codex idle)",
            None,
        );
        assert_eq!(classify(ME, &m, None), Addressing::NotAddressed);
    }

    #[test]
    fn a_mention_of_a_longer_name_is_not_a_mention_of_the_responder() {
        let m = msg("A", "bob", "@rite-dev-2 over to you", None);
        assert_eq!(classify(ME, &m, None), Addressing::NotAddressed);
    }

    #[test]
    fn a_reply_to_the_responders_message_is_addressed() {
        let thread = vec![
            msg("P", ME, "Sync takes the lock first.", None),
            msg("R", "bob", "and then what?", Some("P")),
        ];
        assert_eq!(classify_in_thread(ME, "R", &thread), Addressing::Addressed);
    }

    #[test]
    fn a_reply_to_the_responder_that_mentions_someone_else_is_addressed() {
        let thread = vec![
            msg("P", ME, "Sync takes the lock first.", None),
            msg("R", "bob", "@alice see this", Some("P")),
        ];
        assert_eq!(classify_in_thread(ME, "R", &thread), Addressing::Addressed);
    }

    #[test]
    fn a_reply_in_someone_elses_thread_is_not_addressed() {
        let thread = vec![
            msg("P", "probe-claude", "@probe-codex ping", None),
            msg("R", "probe-codex", "pong", Some("P")),
        ];
        assert_eq!(
            classify_in_thread(ME, "R", &thread),
            Addressing::NotAddressed
        );
    }

    #[test]
    fn a_reply_whose_parent_is_missing_is_unknown() {
        let thread = vec![msg("R", "bob", "and then what?", Some("P"))];
        assert_eq!(classify_in_thread(ME, "R", &thread), Addressing::Unknown);
    }

    #[test]
    fn a_message_missing_from_its_thread_is_unknown() {
        assert_eq!(classify_in_thread(ME, "X", &[]), Addressing::Unknown);
    }

    #[test]
    fn plain_top_level_traffic_is_addressed_to_the_router() {
        let m = msg("A", "bob", "how does sync work?", None);
        assert_eq!(classify(ME, &m, None), Addressing::Addressed);
    }

    #[test]
    fn a_command_prefix_is_addressed_even_with_other_mentions() {
        let m = msg("A", "bob", "!q what did @alice change?", None);
        assert_eq!(classify(ME, &m, None), Addressing::Addressed);
    }

    #[test]
    fn a_dm_with_the_responder_is_addressed_without_fetching() {
        let ids = vec!["A".to_string()];
        let got = resolve(ME, "_dm_bob_rite-dev", &ids, |_| {
            panic!("a DM must not need a fetch")
        });
        assert_eq!(got, Addressing::Addressed);
        assert!(is_dm_with("_dm_rite-dev_zed", ME));
        assert!(!is_dm_with("_dm_alice_bob", ME));
        assert!(!is_dm_with("_dm_bob_rite-dev-2", ME));
        assert!(!is_dm_with("rite", ME));
    }

    #[test]
    fn a_failed_fetch_is_unknown_and_still_posts() {
        let ids = vec!["A".to_string()];
        let got = resolve(ME, "rite", &ids, |_| None);
        assert_eq!(got, Addressing::Unknown);
        assert!(got.posts_failure());
    }

    #[test]
    fn a_batch_is_addressed_if_any_message_is() {
        let thread = vec![
            msg("A", "probe-claude", "@probe-codex ping", None),
            msg("B", "bob", "@rite-dev you there?", None),
        ];
        let ids = vec!["A".to_string(), "B".to_string()];
        let got = resolve(ME, "rite", &ids, |_| Some(thread.clone()));
        assert_eq!(got, Addressing::Addressed);
    }

    #[test]
    fn a_batch_of_other_peoples_pings_is_not_addressed() {
        let thread = vec![
            msg("A", "probe-claude", "@probe-codex ping 1", None),
            msg("B", "probe-claude", "@probe-codex ping 2", None),
        ];
        let ids = vec!["A".to_string(), "B".to_string()];
        let got = resolve(ME, "rite", &ids, |_| Some(thread.clone()));
        assert_eq!(got, Addressing::NotAddressed);
        assert!(!got.posts_failure());
    }

    #[test]
    fn batch_combination_rules() {
        use Addressing::{Addressed, NotAddressed, Unknown};
        assert_eq!(Addressing::of_batch([]), Unknown);
        assert_eq!(Addressing::of_batch([NotAddressed, Unknown]), Unknown);
        assert_eq!(Addressing::of_batch([Unknown, Addressed]), Addressed);
        assert_eq!(
            Addressing::of_batch([NotAddressed, NotAddressed]),
            NotAddressed
        );
    }

    #[test]
    fn thread_json_from_rite_parses() {
        let json = r#"{"messages":[
            {"id":"01M2KF5Y7GC3YQYXDHF2Y6E94Z","agent":"probe-claude","channel":"rite",
             "body":"@probe-codex ping 1","mentions":["probe-codex"],"labels":["probe"]},
            {"id":"01M2KF5Z4N3E0JN4GGGV2RRVWN","agent":"probe-codex","channel":"rite",
             "body":"pong","reply_to":"01M2KF5Y7GC3YQYXDHF2Y6E94Z"}
        ],"thread":{"complete":true}}"#;
        let thread = parse_thread(json).unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(
            classify_in_thread(ME, "01M2KF5Z4N3E0JN4GGGV2RRVWN", &thread),
            Addressing::NotAddressed
        );
    }
}
