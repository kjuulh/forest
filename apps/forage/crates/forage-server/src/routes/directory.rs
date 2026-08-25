//! Machine-facing directory lookup: an external identity in, a person
//! and their linked accounts out.
//!
//! Why this exists: the identity graph is split. Forest owns GitHub and
//! Google (`identities`); Forage owns Slack (`slack_user_links`, because
//! Slack is not in Forest's `OAuthProvider` enum). Everything that could
//! read either was session-gated — fine for a browser, useless to a
//! service. snag wanting to turn the author of a failing dbt model into
//! a Slack mention is the case that surfaced it.
//!
//! Authenticated with a `client_credentials` token, not a session, and
//! gated on the `directory:read` scope. It is the only route in Forage
//! that authenticates that way.
//!
//! ## Why lookup by provider identity matters
//!
//! Email is the obvious key and the wrong one. People commit from
//! addresses their Forest account has never seen — measured across the
//! last 100 commits of one repo, only 19 resolved by email. The linked
//! GitHub identity is exact, and it is keyed on the provider's numeric
//! id rather than a login because logins get renamed.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use forage_core::platform::DirectoryLookup;
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

/// Scope a token must carry. Mirrors `SCOPE_DIRECTORY_READ` in Forest;
/// the authoritative check is Forest's, this is the gate at the door.
const SCOPE_DIRECTORY_READ: &str = "directory:read";

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/directory/resolve", get(resolve))
}

#[derive(Deserialize)]
struct ResolveQuery {
    #[serde(default)]
    email: Option<String>,
    /// `github` | `google`.
    #[serde(default)]
    provider: Option<String>,
    /// The provider's stable id — GitHub's numeric id, not the login.
    #[serde(default)]
    provider_user_id: Option<String>,
}

impl ResolveQuery {
    /// Exactly one lookup key, or an explanation of what was wrong.
    ///
    /// Accepting both and silently preferring one would make a
    /// mis-built query look like a missing person.
    fn into_lookup(self) -> Result<DirectoryLookup, &'static str> {
        let email = self.email.filter(|v| !v.trim().is_empty());
        let provider = self.provider.filter(|v| !v.trim().is_empty());
        let provider_user_id = self.provider_user_id.filter(|v| !v.trim().is_empty());

        match (email, provider, provider_user_id) {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                Err("give either email or provider+provider_user_id, not both")
            }
            (Some(email), None, None) => Ok(DirectoryLookup::Email(email.trim().to_lowercase())),
            (None, Some(provider), Some(provider_user_id)) => Ok(DirectoryLookup::Provider {
                provider: provider.trim().to_ascii_lowercase(),
                provider_user_id: provider_user_id.trim().to_string(),
            }),
            (None, Some(_), None) | (None, None, Some(_)) => {
                Err("provider and provider_user_id must be given together")
            }
            (None, None, None) => Err("give either email or provider+provider_user_id"),
        }
    }
}

async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ResolveQuery>,
) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    };

    let Some(token) = bearer(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "invalid_token");
    };

    // Authenticate before parsing the query: an unauthenticated caller
    // shouldn't learn anything from our validation messages.
    let principal = match client.introspect_client_token(token).await {
        Ok(Some(p)) => p,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_token"),
        Err(e) => {
            tracing::warn!(?e, "directory: token introspection failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    if !principal.scopes.iter().any(|s| s == SCOPE_DIRECTORY_READ) {
        return error(StatusCode::FORBIDDEN, "insufficient_scope");
    }

    let lookup = match query.into_lookup() {
        Ok(l) => l,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_request", "error_description": msg})),
            )
                .into_response();
        }
    };

    let user = match client.resolve_directory_user(lookup).await {
        Ok(Some(u)) => u,
        // Not found is a normal answer here — most commit authors will
        // never have a Forest account — so it is a 200 with `found:
        // false` rather than a 404. Callers loop over people; an
        // exception per miss would be the wrong shape.
        Ok(None) => return Json(json!({"found": false})).into_response(),
        Err(e) => {
            tracing::warn!(?e, "directory: user resolution failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };

    // Slack lives on Forage's side of the graph. No store configured
    // means no Slack half rather than a failed lookup — the Forest half
    // of the answer is still worth returning.
    let slack = match state.integration_store.as_ref() {
        None => Vec::new(),
        Some(store) => match store.list_slack_user_links(&user.user_id).await {
            Ok(links) => links
                .into_iter()
                .map(|l| {
                    json!({
                        "team_id": l.team_id,
                        "team_name": l.team_name,
                        "slack_user_id": l.slack_user_id,
                        "slack_username": l.slack_username,
                    })
                })
                .collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(?e, user_id = %user.user_id, "directory: slack links unreadable");
                Vec::new()
            }
        },
    };

    Json(json!({
        "found": true,
        "user_id": user.user_id,
        "username": user.username,
        "emails": user.emails,
        "slack": slack,
    }))
    .into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// RFC 6750-shaped error, with the challenge header on 401.
fn error(status: StatusCode, code: &'static str) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        return (
            status,
            [(
                axum::http::header::WWW_AUTHENTICATE,
                format!("Bearer error=\"{code}\""),
            )],
            Json(json!({ "error": code })),
        )
            .into_response();
    }
    (status, Json(json!({ "error": code }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(
        email: Option<&str>,
        provider: Option<&str>,
        pid: Option<&str>,
    ) -> Result<DirectoryLookup, &'static str> {
        ResolveQuery {
            email: email.map(str::to_string),
            provider: provider.map(str::to_string),
            provider_user_id: pid.map(str::to_string),
        }
        .into_lookup()
    }

    #[test]
    fn resolves_by_email_normalised() {
        assert_eq!(
            q(Some("  Kasper@Understory.IO "), None, None).unwrap(),
            DirectoryLookup::Email("kasper@understory.io".into())
        );
    }

    #[test]
    fn resolves_by_provider_identity() {
        assert_eq!(
            q(None, Some("GitHub"), Some(" 26280046 ")).unwrap(),
            DirectoryLookup::Provider {
                provider: "github".into(),
                provider_user_id: "26280046".into(),
            }
        );
    }

    /// Silently preferring one key would make a mis-built query look
    /// like a person who isn't there — the most confusing possible
    /// failure for a caller.
    #[test]
    fn giving_both_keys_is_an_error_not_a_preference() {
        assert!(q(Some("a@b.test"), Some("github"), Some("1")).is_err());
        assert!(q(Some("a@b.test"), Some("github"), None).is_err());
    }

    #[test]
    fn half_a_provider_key_is_an_error() {
        assert!(q(None, Some("github"), None).is_err());
        assert!(q(None, None, Some("26280046")).is_err());
    }

    #[test]
    fn no_key_at_all_is_an_error() {
        assert!(q(None, None, None).is_err());
        // Whitespace-only counts as absent, not as a key.
        assert!(q(Some("   "), None, None).is_err());
    }

    #[test]
    fn bearer_extraction_is_strict() {
        let mut h = HeaderMap::new();
        assert_eq!(bearer(&h), None);
        h.insert(axum::http::header::AUTHORIZATION, "token abc".parse().unwrap());
        assert_eq!(bearer(&h), None, "only Bearer is accepted");
        h.insert(axum::http::header::AUTHORIZATION, "Bearer   ".parse().unwrap());
        assert_eq!(bearer(&h), None, "an empty token is no token");
        h.insert(axum::http::header::AUTHORIZATION, "Bearer abc123".parse().unwrap());
        assert_eq!(bearer(&h), Some("abc123"));
    }
}
