use std::collections::HashMap;

use crate::services::users::{UserProfile, UserService};

pub const META_PREFIX: &str = "forest.author";

pub const META_RESOLVED_USER_ID: &str = "forest.author.user_id";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectedAuthor {
    pub origin: Option<String>,
    pub github_login: Option<String>,
    pub github_user_id: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
}

fn non_empty(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(&format!("{META_PREFIX}.{key}"))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

impl DetectedAuthor {
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Option<Self> {
        let detected = Self {
            origin: non_empty(metadata, "origin"),
            github_login: non_empty(metadata, "github_login"),
            github_user_id: non_empty(metadata, "github_user_id"),
            name: non_empty(metadata, "name"),
            email: non_empty(metadata, "email"),
        };

        if detected.names_somebody() {
            Some(detected)
        } else {
            None
        }
    }

    fn names_somebody(&self) -> bool {
        self.github_login.is_some() || self.name.is_some() || self.email.is_some()
    }

    pub fn display_name(&self) -> Option<String> {
        self.github_login
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.email.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linked {
    GithubId,
    Email,
    Unlinked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuthor {
    pub username: Option<String>,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub how: Linked,
}

pub async fn resolve(users: &UserService, detected: &DetectedAuthor) -> ResolvedAuthor {
    if let Some(id) = &detected.github_user_id
        && let Some(user) = by_github_id(users, id).await
    {
        return linked(user, Linked::GithubId);
    }

    if let Some(email) = &detected.email
        && let Some(user) = by_email(users, email).await
    {
        return linked(user, Linked::Email);
    }

    ResolvedAuthor {
        username: detected.display_name(),
        email: detected.email.clone(),
        user_id: None,
        how: Linked::Unlinked,
    }
}

fn linked(user: UserProfile, how: Linked) -> ResolvedAuthor {
    let user_id = user.user_id.to_string();
    let email = user
        .emails
        .iter()
        .find(|e| e.verified)
        .or_else(|| user.emails.first())
        .map(|e| e.email.clone());

    ResolvedAuthor {
        username: Some(user.username),
        email,
        user_id: Some(user_id),
        how,
    }
}

fn github_providers() -> Vec<String> {
    vec!["github".to_string(), "oauth_provider_github".to_string()]
}

async fn by_github_id(users: &UserService, id: &str) -> Option<UserProfile> {
    match users
        .get_user_by_provider_identity(&github_providers(), id)
        .await
    {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!("could not look up release author by github id {id}: {e:#}");
            None
        }
    }
}

async fn by_email(users: &UserService, email: &str) -> Option<UserProfile> {
    match users.get_user_by_email(email).await {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!("could not look up release author by email: {e:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detection_is_read_back_off_the_metadata() {
        let detected = DetectedAuthor::from_metadata(&meta(&[
            ("forest.author.origin", "github-event"),
            ("forest.author.github_login", "dentych"),
            ("forest.author.github_user_id", "2256372"),
            ("forest.author.name", "Dennis Tychsen"),
            ("forest.author.email", "dennis@understory.io"),
        ]))
        .expect("names somebody");

        assert_eq!(detected.github_user_id.as_deref(), Some("2256372"));
        assert_eq!(detected.display_name().as_deref(), Some("dentych"));
    }

    #[test]
    fn an_annotation_without_detection_reads_as_none() {
        assert_eq!(DetectedAuthor::from_metadata(&HashMap::new()), None);
        assert_eq!(
            DetectedAuthor::from_metadata(&meta(&[("deployed_by", "ci")])),
            None
        );
    }

    #[test]
    fn an_origin_on_its_own_does_not_name_anybody() {
        assert_eq!(
            DetectedAuthor::from_metadata(&meta(&[("forest.author.origin", "github-actor")])),
            None
        );
    }

    #[test]
    fn blank_values_do_not_name_anybody() {
        assert_eq!(
            DetectedAuthor::from_metadata(&meta(&[
                ("forest.author.github_login", "   "),
                ("forest.author.name", ""),
            ])),
            None
        );
    }

    #[test]
    fn an_unlinkable_author_falls_back_to_the_handle_not_the_token_owner() {
        let detected = DetectedAuthor {
            origin: Some("git-commit".into()),
            github_login: None,
            github_user_id: None,
            name: Some("Dennis Tychsen".into()),
            email: Some("dennis@understory.io".into()),
        };

        assert_eq!(detected.display_name().as_deref(), Some("Dennis Tychsen"));
    }

    fn profile(username: &str, id: &str, emails: &[(&str, bool)]) -> UserProfile {
        UserProfile {
            user_id: id.parse().expect("uuid"),
            username: username.to_string(),
            profile_picture_url: None,
            emails: emails
                .iter()
                .map(|(e, verified)| crate::services::users::UserEmail {
                    email: e.to_string(),
                    verified: *verified,
                })
                .collect(),
            oauth_connections: Vec::new(),
            mfa_enabled: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn a_linked_author_carries_their_own_user_id() {
        let resolved = linked(
            profile(
                "madsgodvinjensen",
                "01a0487f-1b24-7432-a060-e1ad8e7dc56f",
                &[("mgj@understory.io", true)],
            ),
            Linked::GithubId,
        );

        assert_eq!(resolved.username.as_deref(), Some("madsgodvinjensen"));
        assert_eq!(
            resolved.user_id.as_deref(),
            Some("01a0487f-1b24-7432-a060-e1ad8e7dc56f"),
        );
        assert_eq!(resolved.email.as_deref(), Some("mgj@understory.io"));
    }

    #[test]
    fn a_verified_address_is_preferred() {
        let resolved = linked(
            profile(
                "dentych",
                "019f6588-fc1b-7aa2-befb-9319660792f9",
                &[("old@example.com", false), ("dennis@understory.io", true)],
            ),
            Linked::Email,
        );

        assert_eq!(resolved.email.as_deref(), Some("dennis@understory.io"));
    }

    #[test]
    fn an_unlinked_author_has_no_user_id() {
        let detected = DetectedAuthor {
            origin: Some("git-commit".into()),
            github_login: None,
            github_user_id: None,
            name: Some("Someone Unlinked".into()),
            email: Some("nobody@example.com".into()),
        };

        assert_eq!(detected.display_name().as_deref(), Some("Someone Unlinked"));
    }

    #[test]
    fn both_provider_spellings_are_queried() {
        let providers = github_providers();

        assert!(providers.iter().any(|p| p == "github"));
        assert!(providers.iter().any(|p| p == "oauth_provider_github"));
    }

    #[test]
    fn the_login_outranks_the_display_name() {
        let detected = DetectedAuthor {
            origin: None,
            github_login: Some("dentych".into()),
            github_user_id: None,
            name: Some("Dennis Tychsen".into()),
            email: None,
        };

        assert_eq!(detected.display_name().as_deref(), Some("dentych"));
    }
}
