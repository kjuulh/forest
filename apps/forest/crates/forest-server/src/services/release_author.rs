//! Linking the person who wrote a change to the forest user who is them.
//!
//! An annotation arrives with two different people on it. The **actor** is
//! whoever the token authenticated — in CI, the owner of the secret, which is
//! one person for every repo that shares it. The **author** is whoever wrote
//! the commit. Attributing releases to the actor is what made every deploy of
//! every project read as the same name regardless of who did the work.
//!
//! `forest release annotate --detect` sniffs the author out of the CI
//! environment and records what it found under `forest.author.*` metadata. This
//! module reads that back and tries to turn it into a forest user, so the
//! release view can show the person rather than the string off a commit.
//!
//! Three rungs, and the order is about how badly each can be wrong:
//!
//! 1. **GitHub account id.** Exact and rename-proof, and the CLI only ever
//!    sends one it could prove belongs to the author.
//! 2. **Email.** Exact when it matches, and simply absent when the author
//!    commits under an address forest has never seen — GitHub's
//!    `…@users.noreply.github.com` privacy addresses never match.
//! 3. **Nothing.** Keep the raw sniffed name. "Dennis Tychsen" with no avatar
//!    is a true statement about who wrote the change; the token owner's name
//!    is a false one, and falling back to it is the bug this fixes.

use std::collections::HashMap;

use crate::services::users::{UserProfile, UserService};

/// Metadata keys written by `forest release annotate --detect`. Namespaced
/// because `--metadata` is a free-for-all that projects put their own keys in.
pub const META_PREFIX: &str = "forest.author";

/// Set by the server once it has linked the author to a forest account, so the
/// resolution can be read back rather than re-derived.
pub const META_RESOLVED_USER_ID: &str = "forest.author.user_id";

/// The raw signals `--detect` sniffed, as they arrived.
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
    /// Read the `forest.author.*` keys off an annotation's metadata.
    ///
    /// `None` when the annotation carries no detection at all, which is every
    /// annotation made without `--detect` — the caller keeps its existing
    /// behaviour rather than being handed a blank author.
    pub fn from_metadata(metadata: &HashMap<String, String>) -> Option<Self> {
        let detected = Self {
            origin: non_empty(metadata, "origin"),
            github_login: non_empty(metadata, "github_login"),
            github_user_id: non_empty(metadata, "github_user_id"),
            name: non_empty(metadata, "name"),
            email: non_empty(metadata, "email"),
        };

        // An origin alone is bookkeeping, not an author. Requiring something
        // that actually names a person keeps a detection that found nobody
        // from displacing the fallback.
        if detected.names_somebody() {
            Some(detected)
        } else {
            None
        }
    }

    fn names_somebody(&self) -> bool {
        self.github_login.is_some() || self.name.is_some() || self.email.is_some()
    }

    /// What to show when no forest account could be found. The login first: it
    /// is what a reader recognises and what the avatar route would have
    /// resolved had the account existed.
    pub fn display_name(&self) -> Option<String> {
        self.github_login
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.email.clone())
    }
}

/// How the author was arrived at, for the log line and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linked {
    /// Matched a forest account on the GitHub id.
    GithubId,
    /// Matched a forest account on a verified email.
    Email,
    /// No forest account; the raw sniffed identity is being used as-is.
    Unlinked,
}

/// What the annotation should record about the author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuthor {
    /// `source.username` — a forest username when linked, the raw handle when
    /// not.
    pub username: Option<String>,
    pub email: Option<String>,
    /// The forest user, when one was found. Recorded in metadata rather than in
    /// `source.user_id`, which stays the actor's — see `grpc::release`.
    pub user_id: Option<String>,
    pub how: Linked,
}

/// Turn sniffed signals into a forest identity where possible.
///
/// Never fails the annotation: a database that cannot answer degrades to the
/// unlinked identity, because a release recorded under a plain name is better
/// than a release refused over an avatar.
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
    // Prefer a verified address: this one is shown, and an unverified address
    // is a claim rather than a fact.
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

/// The provider column has been written two ways over the life of the table —
/// the short name and the lowercased enum wire name — and a read that picks one
/// silently misses half the rows. `get_user_by_provider_identity` takes the
/// list for exactly this reason.
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

    /// The shape `--detect` writes for a CI push.
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

    /// An annotation made without `--detect` must read as "no detection" so the
    /// caller keeps its existing behaviour rather than blanking the author.
    #[test]
    fn an_annotation_without_detection_reads_as_none() {
        assert_eq!(DetectedAuthor::from_metadata(&HashMap::new()), None);
        assert_eq!(
            DetectedAuthor::from_metadata(&meta(&[("deployed_by", "ci")])),
            None
        );
    }

    /// `--detect` that found nothing still writes its origin. That is
    /// bookkeeping about a failed sniff, not an author, and must not displace
    /// the fallback.
    #[test]
    fn an_origin_on_its_own_does_not_name_anybody() {
        assert_eq!(
            DetectedAuthor::from_metadata(&meta(&[("forest.author.origin", "github-actor")])),
            None
        );
    }

    /// Blank values are the same as absent — GitHub exports empty strings for
    /// variables it has no value for, and they survive the trip.
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

    /// With no account to link to, the raw sniffed handle is still the truth
    /// about who wrote the change.
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

    /// Both spellings the provider column has been written with must be
    /// asked for. understory/dentych's GitHub link is stored under the newer
    /// one; dropping either half turns "linked" into "no such person", which
    /// is indistinguishable from the truth at the call site.
    #[test]
    fn both_provider_spellings_are_queried() {
        let providers = github_providers();

        assert!(providers.iter().any(|p| p == "github"));
        assert!(providers.iter().any(|p| p == "oauth_provider_github"));
    }

    /// The login is what the avatar route resolves by, so it outranks the
    /// display name when both were sniffed.
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
