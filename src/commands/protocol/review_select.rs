//! Bone-scoped review selection.
//!
//! `seal reviews list` is repo-global: it returns every review in the project,
//! including merged ones belonging to unrelated bones. Picking the first entry
//! therefore let a stale, already-approved review satisfy the review gate for a
//! bone that was never reviewed.
//!
//! A review is tied to a bone only by the `"<bone-id>: <title>"` convention that
//! [`super::shell::seal_create_cmd`] emits, so selection matches on that prefix
//! and skips reviews that are no longer live.

use super::adapters::ReviewSummary;

/// Statuses in which a review can still gate work.
///
/// This is an ALLOW-list, mirroring seal's `ReviewStatus` enum (`Open`,
/// `Approved`, `Merged`, `Abandoned`). Anything else — a merged/abandoned
/// review, a status seal grows later, or the empty string `ReviewSummary::status`
/// defaults to when the field is absent — is not live, so it is never selected
/// and its votes never satisfy the gate.
///
/// The direction matters: a deny-list would treat every unrecognized status as
/// live and let its stale LGTMs open the gate, which is the exact bug class this
/// module exists to kill. An allow-list fails CLOSED — an unknown status jams the
/// gate (visible, recoverable) instead of opening it (silent, unrecoverable).
const LIVE_STATUSES: [&str; 2] = ["open", "approved"];

/// The status seal reports once required reviewers have signed off.
const APPROVED_STATUS: &str = "approved";

/// True when a review is still live and may gate work.
///
/// Unknown and empty statuses are NOT live — see [`LIVE_STATUSES`].
#[must_use]
pub fn is_live_status(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    LIVE_STATUSES.contains(&status.as_str())
}

/// Build a review title that [`title_matches_bone`] accepts for `bone_id`.
///
/// This is the ONLY place review titles are constructed. Every `seal reviews
/// create` command edict emits routes through it (via
/// [`super::shell::seal_create_cmd`]), so a review created by following edict's
/// own guidance is always discoverable by the gate. Passing a title that already
/// carries the prefix is idempotent.
#[must_use]
pub fn scoped_title(bone_id: &str, title: &str) -> String {
    let bone_id = bone_id.trim();
    let title = title.trim();

    if title_matches_bone(Some(title), bone_id) {
        return title.to_string();
    }
    if title.is_empty() {
        return format!("{bone_id}: review");
    }
    format!("{bone_id}: {title}")
}

/// True when a review title names `bone_id` per the `"<bone-id>: <title>"` convention.
///
/// The trailing colon is required so `bn-24r` does not match `bn-24r9: ...`.
#[must_use]
pub fn title_matches_bone(title: Option<&str>, bone_id: &str) -> bool {
    let bone_id = bone_id.trim();
    if bone_id.is_empty() {
        return false;
    }
    let Some(title) = title.map(str::trim_start) else {
        return false;
    };

    title
        .get(..bone_id.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(bone_id))
        && title
            .get(bone_id.len()..)
            .is_some_and(|rest| rest.starts_with(':'))
}

/// All live (non-terminal) reviews belonging to `bone_id`.
#[must_use]
pub fn live_reviews_for_bone<'a>(
    reviews: &'a [ReviewSummary],
    bone_id: &str,
) -> Vec<&'a ReviewSummary> {
    reviews
        .iter()
        .filter(|r| is_live_status(&r.status) && title_matches_bone(r.title.as_deref(), bone_id))
        .collect()
}

/// The review that gates `bone_id`, or `None` when the bone has no live review.
///
/// When a bone has several live reviews (a duplicate was created), the
/// not-yet-approved one wins so the gate stays closed until every review for the
/// bone is signed off.
#[must_use]
pub fn select_for_bone<'a>(
    reviews: &'a [ReviewSummary],
    bone_id: &str,
) -> Option<&'a ReviewSummary> {
    let candidates = live_reviews_for_bone(reviews, bone_id);
    candidates
        .iter()
        .find(|r| !r.status.trim().eq_ignore_ascii_case(APPROVED_STATUS))
        .or_else(|| candidates.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(id: &str, status: &str, title: &str) -> ReviewSummary {
        ReviewSummary {
            review_id: id.into(),
            title: Some(title.into()),
            status: status.into(),
            change_id: None,
            author: None,
        }
    }

    #[test]
    fn live_statuses_are_an_allow_list() {
        // seal's ReviewStatus: Open, Approved, Merged, Abandoned.
        assert!(is_live_status("open"));
        assert!(is_live_status("approved"));
        assert!(is_live_status("APPROVED"));
        assert!(is_live_status("  open  "));
        assert!(!is_live_status("merged"));
        assert!(!is_live_status("abandoned"));
    }

    /// A status edict does not recognize must never gate work. Anything else means
    /// seal adding a state silently re-opens the hole this module exists to close.
    #[test]
    fn unknown_and_empty_statuses_are_not_live() {
        // ReviewSummary::status is #[serde(default)], so an absent field yields "".
        assert!(!is_live_status(""));
        assert!(!is_live_status("   "));
        // Hypothetical future seal states.
        assert!(!is_live_status("rejected"));
        assert!(!is_live_status("superseded"));
        assert!(!is_live_status("draft"));
    }

    /// The empty-status case is the dangerous one: `select_for_bone` prefers the
    /// NOT-approved candidate, so a status-less review would outrank a real one.
    #[test]
    fn unknown_status_review_is_never_selected() {
        let reviews = vec![
            review("cr-unknown", "", "bn-24r: missing status field"),
            review("cr-real", "approved", "bn-24r: the actual review"),
        ];
        let selected = select_for_bone(&reviews, "bn-24r").expect("the approved review is live");
        assert_eq!(
            selected.review_id, "cr-real",
            "an unrecognized status must not be treated as a live review"
        );

        // And on its own it gates nothing at all.
        let only_unknown = vec![review("cr-unknown", "rejected", "bn-24r: unknown state")];
        assert!(select_for_bone(&only_unknown, "bn-24r").is_none());
    }

    /// The HIGH bug this fix exists for: edict emitted `seal reviews create` commands
    /// whose titles its own selector could never match, so a review created by
    /// following edict's guidance was invisible to the gate — approvable but never
    /// found, with `--force` as the only way forward. Every title edict constructs
    /// must round-trip through the matcher.
    #[test]
    fn scoped_title_always_round_trips_through_the_matcher() {
        let bone = "bn-3si2";
        let call_site_titles = [
            // review.rs: raw bone title
            "Protocol review selection bug: unscoped review list",
            // finish.rs: raw bone title, previously passed with no prefix at all
            "Remove jj-related instructions",
            // merge.rs: previously "Work from <id>", which could never match
            "work from bn-3si2",
            // review.rs terminal-review message
            "<title>",
            // degenerate inputs
            "",
            "   ",
            "café ☕",
            "title: with a colon",
        ];

        for raw in call_site_titles {
            let scoped = scoped_title(bone, raw);
            assert!(
                title_matches_bone(Some(&scoped), bone),
                "scoped_title({bone:?}, {raw:?}) = {scoped:?}, which the gate cannot match"
            );
        }
    }

    #[test]
    fn scoped_title_is_idempotent() {
        let already = scoped_title("bn-24r", "bn-24r: already prefixed");
        assert_eq!(already, "bn-24r: already prefixed");
        assert_eq!(scoped_title("bn-24r", &already), already);
        // A bone whose title merely mentions another bone still gets its own prefix.
        assert_eq!(
            scoped_title("bn-24r", "bn-999: someone else"),
            "bn-24r: bn-999: someone else"
        );
    }

    #[test]
    fn title_matches_bone_requires_colon_delimiter() {
        assert!(title_matches_bone(Some("bn-24r: fix the thing"), "bn-24r"));
        // Prefix collision: bn-24r must not match bn-24r9's review.
        assert!(!title_matches_bone(Some("bn-24r9: other bone"), "bn-24r"));
        assert!(!title_matches_bone(Some("bn-24r"), "bn-24r"));
        assert!(!title_matches_bone(
            Some("fix bn-24r: not a prefix"),
            "bn-24r"
        ));
        assert!(!title_matches_bone(None, "bn-24r"));
        assert!(!title_matches_bone(Some("bn-24r: x"), ""));
    }

    #[test]
    fn title_matches_bone_tolerates_leading_space_and_case() {
        assert!(title_matches_bone(Some("  bn-24r: padded"), "bn-24r"));
        assert!(title_matches_bone(Some("BN-24R: shouty"), "bn-24r"));
    }

    #[test]
    fn title_matching_is_not_confused_by_multibyte_titles() {
        assert!(title_matches_bone(Some("bn-24r: café ☕"), "bn-24r"));
        assert!(!title_matches_bone(Some("☕: coffee"), "bn-24r"));
    }

    /// The reported bug: an unrelated merged review must never be selected.
    #[test]
    fn merged_review_for_another_bone_is_not_selected() {
        let reviews = vec![
            review("cr-316fr8", "merged", "bd-3qwo: TOML migration cleanup"),
            review("cr-1uy5", "approved", "bd-vv3l: some other bone"),
        ];
        assert!(select_for_bone(&reviews, "bn-24r").is_none());
    }

    #[test]
    fn merged_review_for_the_same_bone_is_not_selected() {
        // A previous review for this bone already merged — the bone needs a fresh one.
        let reviews = vec![review("cr-old", "merged", "bn-24r: earlier round")];
        assert!(select_for_bone(&reviews, "bn-24r").is_none());
    }

    #[test]
    fn live_review_for_the_bone_is_selected() {
        let reviews = vec![
            review("cr-merged", "merged", "bd-3qwo: unrelated"),
            review("cr-mine", "open", "bn-24r: the actual work"),
        ];
        let selected = select_for_bone(&reviews, "bn-24r").expect("bone has a live review");
        assert_eq!(selected.review_id, "cr-mine");
    }

    #[test]
    fn approved_review_for_the_bone_is_selected() {
        let reviews = vec![review("cr-mine", "approved", "bn-24r: signed off")];
        let selected = select_for_bone(&reviews, "bn-24r").expect("bone has a live review");
        assert_eq!(selected.review_id, "cr-mine");
    }

    #[test]
    fn duplicate_reviews_select_the_unapproved_one() {
        let reviews = vec![
            review("cr-approved", "approved", "bn-24r: first attempt"),
            review("cr-open", "open", "bn-24r: duplicate still open"),
        ];
        let selected = select_for_bone(&reviews, "bn-24r").expect("bone has live reviews");
        assert_eq!(
            selected.review_id, "cr-open",
            "gate must stay closed while any review for the bone is unapproved"
        );
    }

    #[test]
    fn live_reviews_excludes_terminal_and_other_bones() {
        let reviews = vec![
            review("cr-a", "open", "bn-24r: one"),
            review("cr-b", "merged", "bn-24r: two"),
            review("cr-c", "open", "bn-999: three"),
            review("cr-d", "approved", "bn-24r: four"),
        ];
        let live: Vec<&str> = live_reviews_for_bone(&reviews, "bn-24r")
            .iter()
            .map(|r| r.review_id.as_str())
            .collect();
        assert_eq!(live, vec!["cr-a", "cr-d"]);
    }

    #[test]
    fn empty_list_selects_nothing() {
        assert!(select_for_bone(&[], "bn-24r").is_none());
    }
}
