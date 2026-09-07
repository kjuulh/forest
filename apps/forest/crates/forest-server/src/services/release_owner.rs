//! Who, if anybody, gets told about a release *personally*.
//!
//! A release notification goes out on two paths and they answer different
//! questions. The **channel** post is the fleet-wide feed: everything that
//! deploys, in one place, for everybody — that is what the channel is for. A
//! **personal** notification (the `forest notifications` feed, and the Slack DM
//! forage sends off the same row) is addressed to one person, and the only
//! person it is ever addressed to is whoever owns the release.
//!
//! Non-owners are not a matter of taste or of an unset preference. They already
//! see the release in the channel, so a personal notification to them is a
//! second copy of something they have; the default for a non-owner is silence.
//!
//! ## Why the actor was the wrong answer
//!
//! `source.user_id` on an annotation is the **actor**: whichever credential
//! made the call, which in CI is the owner of the shared `FOREST_TOKEN` org
//! secret — one person for every repo. Where `--detect` resolved the commit
//! author to a forest account, `grpc::release` moves the author's id onto
//! `source` and the two agree. Where it did not — `renovate[bot]` has no forest
//! account and never will — `source.user_id` stays the token owner, so copying
//! it into the notification DM'd whoever holds the secret about a change a bot
//! wrote. That is the bug this module exists to keep fixed: an unresolved
//! author means *nobody*, never the actor.
//!
//! ## The rungs
//!
//! 1. **A linked author** (`forest.author.user_id`, written by `grpc::release`
//!    once `release_author::resolve` found an account). That is the owner.
//! 2. **A detected author that did not link.** The annotation says who wrote
//!    the change and forest cannot map them to a user: a bot, or a human with
//!    no forest account. There is no owner to notify — stop, do not fall back.
//! 3. **No detection at all.** Nothing claimed an author, so the human who ran
//!    `forest release annotate` is the closest thing to one, and for someone
//!    releasing from their own machine it is exactly right.
//! 4. **A machine credential with no detection.** An app token or service
//!    account annotating without saying who for. No human owner, so nobody.

use std::collections::HashMap;

use crate::services::release_author::{DetectedAuthor, META_PREFIX, META_RESOLVED_USER_ID};

/// The `actor_type` recorded on an annotation for a human credential — a user
/// JWT or a personal access token. `Actor::actor_type` is the writer.
const ACTOR_TYPE_USER: &str = "user";

/// Logins GitHub hands to machines. App authors always arrive suffixed —
/// `renovate[bot]`, `dependabot[bot]`, `github-actions[bot]` — and the bare
/// spellings show up on the commit trailer some of them write.
///
/// This is belt-and-braces: rung 2 below already refuses an author it could not
/// link, and a bot has no forest account to link to. It matters for the one case
/// rung 2 misses — a bot that *does* have an account, because somebody signed
/// one up for it once — where an id that resolves is still not a person to DM.
const BOT_LOGINS: &[&str] = &["renovate", "dependabot", "github-actions"];

/// Whether a GitHub login belongs to a machine rather than a person.
pub fn is_bot_login(login: &str) -> bool {
    let login = login.trim().to_ascii_lowercase();
    login.ends_with("[bot]") || BOT_LOGINS.contains(&login.as_str())
}

/// Why a release has no personal recipient. Logged, so a release that went out
/// with nobody DM'd can be told apart from one that failed to look anybody up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoOwner {
    /// A bot wrote the change. Nothing to DM, and no human stands in for it.
    AuthorIsBot,
    /// The annotation named an author forest could not link to an account.
    AuthorUnlinked,
    /// No detection, and the credential that annotated was not a person.
    ActorIsMachine,
    /// No detection and no actor id — an annotation from before either existed.
    Unknown,
}

/// The one user a release notification may be delivered to personally.
///
/// `Err` is not a failure: it is a release that legitimately has nobody to tell,
/// and the channel post still carries it to everyone.
pub type PersonalRecipient = Result<String, NoOwner>;

/// Work out the personal recipient from what the annotation recorded.
///
/// `metadata` is the annotation's metadata (where `--detect` writes its
/// findings and the server writes the account it linked them to), `source_user_id`
/// is `source.user_id` off the annotation, and `actor_type` is the credential
/// kind from `annotations.actor_type`.
pub fn personal_recipient(
    metadata: &HashMap<String, String>,
    source_user_id: Option<&str>,
    actor_type: Option<&str>,
) -> PersonalRecipient {
    let detected = DetectedAuthor::from_metadata(metadata);

    // A bot is a bot whether or not it linked, so this is asked before the id.
    if let Some(login) = detected.as_ref().and_then(|d| d.github_login.as_deref())
        && is_bot_login(login)
    {
        return Err(NoOwner::AuthorIsBot);
    }

    // 1. The author, linked to a forest account.
    if let Some(user_id) = metadata
        .get(META_RESOLVED_USER_ID)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return Ok(user_id.to_string());
    }

    // 2. An author forest could not link. The release has an author and it is
    //    not anybody forest can address — the actor is a different person and
    //    notifying them is the bug, not the fallback.
    if detected.is_some() {
        return Err(NoOwner::AuthorUnlinked);
    }

    // 3./4. Nothing claimed an author, so the credential that annotated is the
    //       best answer — but only when it belongs to a person.
    match (actor_type, source_user_id.map(str::trim).filter(|v| !v.is_empty())) {
        (Some(ACTOR_TYPE_USER), Some(user_id)) => Ok(user_id.to_string()),
        (Some(ACTOR_TYPE_USER), None) | (None, _) => Err(NoOwner::Unknown),
        (Some(_), _) => Err(NoOwner::ActorIsMachine),
    }
}

/// The same decision, flattened for the notification context, which carries
/// "nobody" as an absent id.
pub fn personal_recipient_id(
    metadata: &HashMap<String, String>,
    source_user_id: Option<&str>,
    actor_type: Option<&str>,
) -> Option<String> {
    match personal_recipient(metadata, source_user_id, actor_type) {
        Ok(user_id) => Some(user_id),
        Err(reason) => {
            tracing::debug!(
                ?reason,
                author = metadata
                    .get(&format!("{META_PREFIX}.github_login"))
                    .map(String::as_str)
                    .unwrap_or("?"),
                "release has no personal recipient — channel only"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KASPER: &str = "01a0487f-1b24-7432-a060-e1ad8e7dc56f";
    const DENNIS: &str = "019f6588-fc1b-7aa2-befb-9319660792f9";

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The screenshot's case, and the reason this module exists: a
    /// renovate[bot] PR annotated by CI, where `source.user_id` is whoever owns
    /// the shared `FOREST_TOKEN`. Kasper is not the author of anything here.
    #[test]
    fn a_bot_authored_release_has_no_personal_recipient() {
        let recipient = personal_recipient(
            &meta(&[
                ("forest.author.origin", "github-event"),
                ("forest.author.github_login", "renovate[bot]"),
                ("forest.author.github_user_id", "29139614"),
                ("forest.author.name", "renovate[bot]"),
            ]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Err(NoOwner::AuthorIsBot));
    }

    /// Bot or not, an author forest could not link is not the actor. A human
    /// contributor with no forest account must not redirect their release's DM
    /// to whoever owns the CI token.
    #[test]
    fn an_unlinked_human_author_does_not_fall_back_to_the_actor() {
        let recipient = personal_recipient(
            &meta(&[
                ("forest.author.origin", "git-commit"),
                ("forest.author.name", "Someone Unlinked"),
                ("forest.author.email", "nobody@example.com"),
            ]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Err(NoOwner::AuthorUnlinked));
    }

    /// A linked author owns their release, and the id is theirs — not the
    /// token's.
    #[test]
    fn a_linked_author_is_the_recipient() {
        let recipient = personal_recipient(
            &meta(&[
                ("forest.author.github_login", "dentych"),
                ("forest.author.github_user_id", "2256372"),
                ("forest.author.user_id", DENNIS),
            ]),
            // The actor is still recorded, and is still somebody else.
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Ok(DENNIS.to_string()));
    }

    /// Somebody running `forest release annotate` from their own machine. No
    /// detection, so the person who ran it is the owner — this is the
    /// pre-existing behaviour and it stays.
    #[test]
    fn without_detection_the_human_who_annotated_owns_it() {
        let recipient = personal_recipient(&HashMap::new(), Some(KASPER), Some("user"));

        assert_eq!(recipient, Ok(KASPER.to_string()));
    }

    /// An app token or service account annotating without saying who for. There
    /// is no person on this release, and the app's id is not one.
    #[test]
    fn a_machine_credential_with_no_detection_owns_nothing() {
        for actor_type in ["app", "service_account"] {
            assert_eq!(
                personal_recipient(&HashMap::new(), Some(KASPER), Some(actor_type)),
                Err(NoOwner::ActorIsMachine),
                "{actor_type} should not be notified personally",
            );
        }
    }

    /// `--detect` that found nobody writes its origin and nothing else.
    /// `DetectedAuthor` reads that as no detection, so rung 3 applies — the
    /// person who ran the command.
    #[test]
    fn an_origin_alone_is_not_an_author() {
        let recipient = personal_recipient(
            &meta(&[("forest.author.origin", "github-actor")]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Ok(KASPER.to_string()));
    }

    /// Rows written before any of this existed: no metadata, no actor. Nobody
    /// to address, so nobody is addressed.
    #[test]
    fn an_annotation_with_nothing_recorded_has_no_recipient() {
        assert_eq!(
            personal_recipient(&HashMap::new(), None, None),
            Err(NoOwner::Unknown),
        );
    }

    /// A bot that somebody once signed a forest account up for still is not a
    /// person to DM, so the bot check runs before the linked id.
    #[test]
    fn a_bot_with_an_account_is_still_a_bot() {
        let recipient = personal_recipient(
            &meta(&[
                ("forest.author.github_login", "dependabot[bot]"),
                ("forest.author.user_id", DENNIS),
            ]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Err(NoOwner::AuthorIsBot));
    }

    #[test]
    fn bot_logins_are_recognised_in_both_spellings() {
        assert!(is_bot_login("renovate[bot]"));
        assert!(is_bot_login("Renovate[Bot]"));
        assert!(is_bot_login("dependabot"));
        assert!(is_bot_login("github-actions[bot]"));
        assert!(!is_bot_login("dentych"));
        assert!(!is_bot_login("kjuulh"));
        // A person is not a bot for having "bot" in their handle.
        assert!(!is_bot_login("botond"));
    }

    /// The flattened form is what the notification context stores: an id or
    /// nothing.
    #[test]
    fn the_flattened_form_carries_nobody_as_none() {
        assert_eq!(
            personal_recipient_id(&HashMap::new(), Some(KASPER), Some("user")),
            Some(KASPER.to_string()),
        );
        assert_eq!(
            personal_recipient_id(
                &meta(&[("forest.author.github_login", "renovate[bot]")]),
                Some(KASPER),
                Some("user"),
            ),
            None,
        );
    }
}
