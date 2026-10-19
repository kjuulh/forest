use std::collections::HashMap;

use crate::services::release_author::{DetectedAuthor, META_PREFIX, META_RESOLVED_USER_ID};

const ACTOR_TYPE_USER: &str = "user";

const BOT_LOGINS: &[&str] = &["renovate", "dependabot", "github-actions"];

pub fn is_bot_login(login: &str) -> bool {
    let login = login.trim().to_ascii_lowercase();
    login.ends_with("[bot]") || BOT_LOGINS.contains(&login.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoOwner {
    AuthorIsBot,
    AuthorUnlinked,
    ActorIsMachine,
    Unknown,
}

pub type PersonalRecipient = Result<String, NoOwner>;

pub fn personal_recipient(
    metadata: &HashMap<String, String>,
    source_user_id: Option<&str>,
    actor_type: Option<&str>,
) -> PersonalRecipient {
    let detected = DetectedAuthor::from_metadata(metadata);

    if let Some(login) = detected.as_ref().and_then(|d| d.github_login.as_deref())
        && is_bot_login(login)
    {
        return Err(NoOwner::AuthorIsBot);
    }

    if let Some(user_id) = metadata
        .get(META_RESOLVED_USER_ID)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return Ok(user_id.to_string());
    }

    if detected.is_some() {
        return Err(NoOwner::AuthorUnlinked);
    }

    match (
        actor_type,
        source_user_id.map(str::trim).filter(|v| !v.is_empty()),
    ) {
        (Some(ACTOR_TYPE_USER), Some(user_id)) => Ok(user_id.to_string()),
        (Some(ACTOR_TYPE_USER), None) | (None, _) => Err(NoOwner::Unknown),
        (Some(_), _) => Err(NoOwner::ActorIsMachine),
    }
}

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

    #[test]
    fn a_linked_author_is_the_recipient() {
        let recipient = personal_recipient(
            &meta(&[
                ("forest.author.github_login", "dentych"),
                ("forest.author.github_user_id", "2256372"),
                ("forest.author.user_id", DENNIS),
            ]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Ok(DENNIS.to_string()));
    }

    #[test]
    fn without_detection_the_human_who_annotated_owns_it() {
        let recipient = personal_recipient(&HashMap::new(), Some(KASPER), Some("user"));

        assert_eq!(recipient, Ok(KASPER.to_string()));
    }

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

    #[test]
    fn an_origin_alone_is_not_an_author() {
        let recipient = personal_recipient(
            &meta(&[("forest.author.origin", "github-actor")]),
            Some(KASPER),
            Some("user"),
        );

        assert_eq!(recipient, Ok(KASPER.to_string()));
    }

    #[test]
    fn an_annotation_with_nothing_recorded_has_no_recipient() {
        assert_eq!(
            personal_recipient(&HashMap::new(), None, None),
            Err(NoOwner::Unknown),
        );
    }

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
        assert!(!is_bot_login("botond"));
    }

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
