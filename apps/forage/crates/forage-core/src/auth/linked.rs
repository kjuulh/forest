//! Account-linking types and pure mappers for external identity providers.
//!
//! Forest is the source of truth for GitHub/Google OAuth identities (via the
//! `identities` table and `LinkOAuthProvider` / `UnlinkOAuthProvider` RPCs).
//! Slack is sourced from Forage's `slack_user_links` table because Slack is
//! not in Forest's `OAuthProvider` enum.
//!
//! This module holds:
//! - `LinkedProvider`: provider identifier for the unified UI view
//! - `LinkedIdentity`: render model used by the account page
//! - `LinkOAuthInput`: pure value object representing a request to link an
//!   identity in Forest (decoupled from gRPC types)
//! - Pure mappers from `OidcIdentity` (already-verified provider profile)
//!   into `LinkOAuthInput`

use serde::{Deserialize, Serialize};

use crate::auth::OidcIdentity;
use crate::integrations::SlackUserLink;

/// Provider identifier for a linked external account.
///
/// `GitHub` and `Google` are sourced from Forest's `identities`; `Slack` is
/// sourced from Forage's `slack_user_links`. The unified view-model merges
/// both into a single `Vec<LinkedIdentity>` for the account page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkedProvider {
    GitHub,
    Google,
    Slack,
}

impl LinkedProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::Google => "google",
            Self::Slack => "slack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "github" => Some(Self::GitHub),
            "google" => Some(Self::Google),
            "slack" => Some(Self::Slack),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Google => "Google",
            Self::Slack => "Slack",
        }
    }
}

/// A single linked external identity, rendered on the account page.
///
/// `external_id` is the provider's stable identifier (GitHub numeric id,
/// Google `sub`, Slack user id). `display_name` is the human-readable label
/// — login for GitHub, name/email for Google, slack_username for Slack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedIdentity {
    pub provider: LinkedProvider,
    pub external_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    /// ISO-8601 timestamp from Forest (`linked_at`) or Forage (`created_at`).
    /// Optional because some sources may not surface it.
    pub linked_at: Option<String>,
    /// Provider-specific subtitle (e.g. Slack team name). None for
    /// providers where the display name already conveys identity.
    pub subtitle: Option<String>,
    /// Provider-specific key used by the disconnect form. For Slack this
    /// is the workspace `team_id` (the existing `/settings/account/slack/disconnect`
    /// route expects `team_id`); for GitHub/Google it is unused (the
    /// disconnect route derives identity from the session).
    pub disconnect_key: Option<String>,
}

/// Drop avatar URLs that aren't HTTPS. The login flow already does this
/// (see `forest_client.rs`), but the link flow used to copy the value
/// through unfiltered — meaning a `javascript:` or `http://` URL could
/// be stored in `provider_data` JSONB and surface in the UI later. The
/// current template renders provider icons as static SVG, but a future
/// avatar-rendering change would expose the gap. Symmetric to the
/// login-flow validation closes the asymmetry caught in adversarial
/// review gap #6.
///
/// Case-insensitive scheme match: RFC 3986 says URL schemes are
/// case-insensitive. Real OAuth providers always return lowercase, but
/// being lenient here costs nothing and makes the check robust if a
/// future provider returns `HTTPS://`.
fn filter_avatar_url(url: Option<&str>) -> Option<String> {
    url.filter(|u| {
        u.find("://")
            .map(|end| u[..end].eq_ignore_ascii_case("https"))
            .unwrap_or(false)
    })
    .map(String::from)
}

/// Pure value object representing a `LinkOAuthProvider` request to Forest.
/// `provider_data_json` carries the un-modelled extras (avatar, login) as
/// raw JSON so they can be round-tripped through Forest's `provider_data`
/// JSONB column without a schema change on Forage's side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkOAuthInput {
    pub provider: LinkedProvider,
    pub provider_user_id: String,
    pub provider_email: String,
    pub provider_display_name: String,
    pub provider_data_json: String,
}

/// Provider-data JSON shape we write into Forest's `identities.provider_data`.
/// Kept intentionally small and provider-agnostic so the rendering code
/// doesn't need to know which provider produced it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProviderDataExtras {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Map an OIDC identity (already-verified profile from the provider) into
/// the input for Forest's `LinkOAuthProvider` RPC. The caller has already
/// validated the email via OIDC `email_verified` — this is a pure shape
/// transformation.
///
/// Display-name precedence: `identity.login` (handle) > `identity.name` >
/// `identity.email`. The login takes priority because providers like
/// GitHub use it as the user-recognisable identifier, even when a
/// "real name" is also populated.
pub fn link_input_from_oidc(
    provider: LinkedProvider,
    identity: &OidcIdentity,
) -> LinkOAuthInput {
    let extras = ProviderDataExtras {
        login: identity.login.clone(),
        avatar_url: filter_avatar_url(identity.picture_url.as_deref()),
        name: if identity.name.is_empty() {
            None
        } else {
            Some(identity.name.clone())
        },
    };
    let display_name = identity
        .login
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if identity.name.is_empty() {
                identity.email.clone()
            } else {
                identity.name.clone()
            }
        });
    LinkOAuthInput {
        provider,
        provider_user_id: identity.sub.clone(),
        provider_email: identity.email.clone(),
        provider_display_name: display_name,
        provider_data_json: serde_json::to_string(&extras).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// GitHub OIDC exchanges in this codebase populate `sub` with GitHub's
/// numeric user id and `name` with the user's display name. GitHub doesn't
/// expose a `login` (handle) through OIDC, so callers fetching `/user`
/// can pass it via `github_login_override`.
pub fn link_input_from_github(
    identity: &OidcIdentity,
    github_login_override: Option<&str>,
) -> LinkOAuthInput {
    let extras = ProviderDataExtras {
        login: github_login_override.map(|s| s.to_string()),
        avatar_url: filter_avatar_url(identity.picture_url.as_deref()),
        name: if identity.name.is_empty() {
            None
        } else {
            Some(identity.name.clone())
        },
    };
    let display_name = github_login_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            if identity.name.is_empty() {
                identity.email.clone()
            } else {
                identity.name.clone()
            }
        });
    LinkOAuthInput {
        provider: LinkedProvider::GitHub,
        provider_user_id: identity.sub.clone(),
        provider_email: identity.email.clone(),
        provider_display_name: display_name,
        provider_data_json: serde_json::to_string(&extras).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Build a `LinkedIdentity` from Forest's `OAuthConnection`-like fields
/// plus the optional provider-data extras decoded from JSONB. Pure
/// function — no I/O.
pub fn linked_identity_from_forest(
    provider: LinkedProvider,
    provider_user_id: &str,
    provider_email: Option<&str>,
    linked_at: Option<&str>,
    extras: Option<&ProviderDataExtras>,
) -> LinkedIdentity {
    // Choose a display name: prefer login (GitHub handle) -> name -> email
    // -> external id as final fallback.
    let display_name = extras
        .and_then(|e| e.login.clone())
        .or_else(|| extras.and_then(|e| e.name.clone()))
        .or_else(|| provider_email.map(|s| s.to_string()))
        .unwrap_or_else(|| provider_user_id.to_string());

    LinkedIdentity {
        provider,
        external_id: provider_user_id.to_string(),
        display_name,
        email: provider_email.map(|s| s.to_string()),
        avatar_url: extras.and_then(|e| e.avatar_url.clone()),
        linked_at: linked_at.map(|s| s.to_string()),
        subtitle: None,
        disconnect_key: None,
    }
}

/// Build a `LinkedIdentity` from a Slack user link. Pure function.
pub fn linked_identity_from_slack(link: &SlackUserLink) -> LinkedIdentity {
    LinkedIdentity {
        provider: LinkedProvider::Slack,
        external_id: link.slack_user_id.clone(),
        display_name: format!("@{}", link.slack_username),
        email: None,
        avatar_url: None,
        linked_at: Some(link.created_at.clone()),
        subtitle: Some(link.team_name.clone()),
        disconnect_key: Some(link.team_id.clone()),
    }
}

/// Merge Forest-sourced identities (github/google) with Slack links into
/// a single render list, in stable order: GitHub, Google, then Slack
/// workspaces by creation time. Pure function.
pub fn merge_linked_identities(
    forest_identities: Vec<LinkedIdentity>,
    slack_links: &[SlackUserLink],
) -> Vec<LinkedIdentity> {
    let mut out = Vec::with_capacity(forest_identities.len() + slack_links.len());
    // GitHub first, then Google, then any other Forest providers.
    let mut github = Vec::new();
    let mut google = Vec::new();
    let mut other = Vec::new();
    for id in forest_identities {
        match id.provider {
            LinkedProvider::GitHub => github.push(id),
            LinkedProvider::Google => google.push(id),
            _ => other.push(id),
        }
    }
    out.extend(github);
    out.extend(google);
    out.extend(other);
    for l in slack_links {
        out.push(linked_identity_from_slack(l));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_oidc() -> OidcIdentity {
        OidcIdentity {
            sub: "12345".to_string(),
            email: "kasper@understory.io".to_string(),
            name: "Kasper Hermansen".to_string(),
            picture_url: Some("https://example.com/avatar.png".to_string()),
            login: None,
        }
    }

    #[test]
    fn linked_provider_roundtrip() {
        for p in &[
            LinkedProvider::GitHub,
            LinkedProvider::Google,
            LinkedProvider::Slack,
        ] {
            assert_eq!(LinkedProvider::parse(p.as_str()), Some(*p));
        }
    }

    #[test]
    fn linked_provider_unknown_returns_none() {
        assert_eq!(LinkedProvider::parse("discord"), None);
        assert_eq!(LinkedProvider::parse(""), None);
    }

    #[test]
    fn link_input_from_oidc_populates_all_fields() {
        let input = link_input_from_oidc(LinkedProvider::Google, &sample_oidc());
        assert_eq!(input.provider, LinkedProvider::Google);
        assert_eq!(input.provider_user_id, "12345");
        assert_eq!(input.provider_email, "kasper@understory.io");
        assert_eq!(input.provider_display_name, "Kasper Hermansen");
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.avatar_url.as_deref(), Some("https://example.com/avatar.png"));
        assert_eq!(extras.name.as_deref(), Some("Kasper Hermansen"));
        assert_eq!(extras.login, None);
    }

    #[test]
    fn link_input_from_oidc_drops_non_https_avatar() {
        // The login flow already filters to https-only — the link flow
        // must match (adversarial review #6).
        let id = OidcIdentity {
            picture_url: Some("javascript:alert(1)".into()),
            ..sample_oidc()
        };
        let input = link_input_from_oidc(LinkedProvider::Google, &id);
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.avatar_url, None);

        let id = OidcIdentity {
            picture_url: Some("http://insecure.example.com/a.png".into()),
            ..sample_oidc()
        };
        let input = link_input_from_oidc(LinkedProvider::Google, &id);
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.avatar_url, None);
    }

    #[test]
    fn link_input_from_oidc_accepts_uppercase_https_scheme() {
        // RFC 3986 says schemes are case-insensitive. Be lenient.
        let id = OidcIdentity {
            picture_url: Some("HTTPS://example.com/a.png".into()),
            ..sample_oidc()
        };
        let input = link_input_from_oidc(LinkedProvider::Google, &id);
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(
            extras.avatar_url.as_deref(),
            Some("HTTPS://example.com/a.png")
        );
    }

    #[test]
    fn link_input_from_github_drops_non_https_avatar() {
        let id = OidcIdentity {
            picture_url: Some("http://insecure.example.com/a.png".into()),
            ..sample_oidc()
        };
        let input = link_input_from_github(&id, Some("kjuulh"));
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.avatar_url, None);
    }

    #[test]
    fn link_input_from_oidc_empty_name_omits_name() {
        let id = OidcIdentity {
            name: String::new(),
            ..sample_oidc()
        };
        let input = link_input_from_oidc(LinkedProvider::Google, &id);
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.name, None);
    }

    #[test]
    fn link_input_from_github_uses_login_for_display_when_provided() {
        let input = link_input_from_github(&sample_oidc(), Some("kjuulh"));
        assert_eq!(input.provider, LinkedProvider::GitHub);
        assert_eq!(input.provider_display_name, "kjuulh");
        let extras: ProviderDataExtras =
            serde_json::from_str(&input.provider_data_json).unwrap();
        assert_eq!(extras.login.as_deref(), Some("kjuulh"));
    }

    #[test]
    fn link_input_from_github_falls_back_to_name_then_email() {
        // No login override, with name -> name wins
        let input = link_input_from_github(&sample_oidc(), None);
        assert_eq!(input.provider_display_name, "Kasper Hermansen");

        // No login override, no name -> email
        let id = OidcIdentity {
            name: String::new(),
            ..sample_oidc()
        };
        let input = link_input_from_github(&id, None);
        assert_eq!(input.provider_display_name, "kasper@understory.io");
    }

    #[test]
    fn linked_identity_from_forest_prefers_login_then_name_then_email() {
        let extras_with_login = ProviderDataExtras {
            login: Some("kjuulh".into()),
            name: Some("Kasper Hermansen".into()),
            avatar_url: Some("https://example.com/a.png".into()),
        };
        let id = linked_identity_from_forest(
            LinkedProvider::GitHub,
            "12345",
            Some("kasper@understory.io"),
            Some("2026-05-20T10:00:00Z"),
            Some(&extras_with_login),
        );
        assert_eq!(id.display_name, "kjuulh");
        assert_eq!(id.email.as_deref(), Some("kasper@understory.io"));
        assert_eq!(id.avatar_url.as_deref(), Some("https://example.com/a.png"));
        assert_eq!(id.linked_at.as_deref(), Some("2026-05-20T10:00:00Z"));
        assert_eq!(id.external_id, "12345");

        let extras_no_login = ProviderDataExtras {
            login: None,
            name: Some("Kasper".into()),
            avatar_url: None,
        };
        let id = linked_identity_from_forest(
            LinkedProvider::Google,
            "g-sub",
            Some("a@b.com"),
            None,
            Some(&extras_no_login),
        );
        assert_eq!(id.display_name, "Kasper");

        let id = linked_identity_from_forest(
            LinkedProvider::Google,
            "g-sub",
            Some("a@b.com"),
            None,
            None,
        );
        assert_eq!(id.display_name, "a@b.com");
    }

    #[test]
    fn linked_identity_from_forest_falls_back_to_external_id_when_nothing_else() {
        let id = linked_identity_from_forest(LinkedProvider::GitHub, "12345", None, None, None);
        assert_eq!(id.display_name, "12345");
        assert_eq!(id.email, None);
    }

    #[test]
    fn linked_identity_from_slack_uses_at_prefixed_username_and_team_subtitle() {
        let link = SlackUserLink {
            id: "uuid".into(),
            user_id: "user-1".into(),
            team_id: "T123".into(),
            team_name: "rawpotion".into(),
            slack_user_id: "U456".into(),
            slack_username: "kjuulh".into(),
            created_at: "2026-05-01T00:00:00Z".into(),
        };
        let id = linked_identity_from_slack(&link);
        assert_eq!(id.provider, LinkedProvider::Slack);
        assert_eq!(id.display_name, "@kjuulh");
        assert_eq!(id.subtitle.as_deref(), Some("rawpotion"));
        assert_eq!(id.external_id, "U456");
        assert_eq!(id.linked_at.as_deref(), Some("2026-05-01T00:00:00Z"));
        // disconnect_key carries team_id so the existing slack_disconnect
        // form receives the right value.
        assert_eq!(id.disconnect_key.as_deref(), Some("T123"));
    }

    #[test]
    fn merge_orders_github_first_then_google_then_slack() {
        let google = LinkedIdentity {
            provider: LinkedProvider::Google,
            external_id: "g".into(),
            display_name: "g".into(),
            email: None,
            avatar_url: None,
            linked_at: None,
            subtitle: None,
            disconnect_key: None,
        };
        let github = LinkedIdentity {
            provider: LinkedProvider::GitHub,
            external_id: "gh".into(),
            display_name: "gh".into(),
            email: None,
            avatar_url: None,
            linked_at: None,
            subtitle: None,
            disconnect_key: None,
        };
        let slack = SlackUserLink {
            id: "s".into(),
            user_id: "u".into(),
            team_id: "T".into(),
            team_name: "rawpotion".into(),
            slack_user_id: "U".into(),
            slack_username: "kj".into(),
            created_at: "t".into(),
        };

        // Pass Forest list in google-first order; output must still be github-first.
        let merged = merge_linked_identities(vec![google, github], &[slack]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].provider, LinkedProvider::GitHub);
        assert_eq!(merged[1].provider, LinkedProvider::Google);
        assert_eq!(merged[2].provider, LinkedProvider::Slack);
    }

    #[test]
    fn merge_handles_empty_inputs() {
        let merged = merge_linked_identities(vec![], &[]);
        assert!(merged.is_empty());
    }

    #[test]
    fn provider_data_extras_skip_none_in_json() {
        let extras = ProviderDataExtras {
            login: Some("kjuulh".into()),
            avatar_url: None,
            name: None,
        };
        let json = serde_json::to_string(&extras).unwrap();
        assert!(json.contains("kjuulh"));
        assert!(!json.contains("avatar_url"));
        assert!(!json.contains("\"name\""));
    }

    #[test]
    fn linked_identity_serde_roundtrip() {
        let id = LinkedIdentity {
            provider: LinkedProvider::GitHub,
            external_id: "12345".into(),
            display_name: "kjuulh".into(),
            email: Some("k@example.com".into()),
            avatar_url: Some("https://example.com/a.png".into()),
            linked_at: Some("2026-05-20T10:00:00Z".into()),
            subtitle: None,
            disconnect_key: None,
        };
        let json = serde_json::to_string(&id).unwrap();
        let parsed: LinkedIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }
}
