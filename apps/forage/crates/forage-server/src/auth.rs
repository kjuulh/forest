use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::Cookie;

use forage_core::session::{CachedOrg, CachedUser, SessionId};

use crate::state::AppState;

pub const SESSION_COOKIE: &str = "forage_session";

/// Maximum access token lifetime: 24 hours.
/// Defends against forest-server returning absolute timestamps instead of durations.
const MAX_TOKEN_LIFETIME_SECS: i64 = 86400;

/// Cap expires_in_seconds to a sane maximum.
pub fn cap_token_expiry(expires_in_seconds: i64) -> i64 {
    expires_in_seconds.min(MAX_TOKEN_LIFETIME_SECS)
}

/// Active session data available to route handlers.
pub struct Session {
    pub session_id: SessionId,
    pub access_token: String,
    pub user: CachedUser,
    pub csrf_token: String,
    pub needs_username: bool,
    pub session_data: forage_core::session::SessionData,
}

/// Validate a `return_to` path as safe to redirect to.
///
/// Accepts only same-origin absolute paths. Rejects:
///   - empty / `None`
///   - anything not starting with `/`
///   - protocol-relative URLs (`//evil.com`) and `/\evil`
///   - absolute URLs (`https://…`) — these don't start with `/`, caught above
pub fn safe_return_to(raw: Option<&str>) -> Option<&str> {
    let p = raw?;
    if p.len() < 2 || !p.starts_with('/') {
        return None;
    }
    let second = p.as_bytes()[1];
    if second == b'/' || second == b'\\' {
        return None;
    }
    Some(p)
}

/// Build a /login redirect that preserves the original URL as a return_to parameter.
fn login_redirect(uri: &axum::http::Uri) -> axum::response::Redirect {
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    if path == "/" || path == "/dashboard" || path.starts_with("/login") || path.starts_with("/signup") {
        axum::response::Redirect::to("/login")
    } else {
        axum::response::Redirect::to(&format!(
            "/login?return_to={}",
            urlencoding::encode(path)
        ))
    }
}

/// Extractor that requires an active session. Redirects to /login if not authenticated.
/// Handles transparent token refresh when access token is near expiry.
impl FromRequestParts<AppState> for Session {
    type Rejection = axum::response::Redirect;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let redirect = login_redirect(&parts.uri);
        let jar = CookieJar::from_headers(&parts.headers);
        let session_id = jar
            .get(SESSION_COOKIE)
            .map(|c| SessionId::from_raw(c.value().to_string()))
            .ok_or(redirect.clone())?;

        let mut session_data = state
            .sessions
            .get(&session_id)
            .await
            .ok()
            .flatten()
            .ok_or(redirect.clone())?;

        // Transparent token refresh
        if session_data.needs_refresh() {
            match state
                .forest_client
                .refresh_token(&session_data.refresh_token)
                .await
            {
                Ok(tokens) => {
                    session_data.access_token = tokens.access_token;
                    session_data.refresh_token = tokens.refresh_token;
                    session_data.access_expires_at =
                        chrono::Utc::now() + chrono::Duration::seconds(cap_token_expiry(tokens.expires_in_seconds));
                    session_data.last_seen_at = chrono::Utc::now();

                    // Refresh the user cache too
                    if let Ok(user) = state
                        .forest_client
                        .get_user(&session_data.access_token)
                        .await
                    {
                        // Preserve existing orgs on failure — a transient gRPC error
                        // should not wipe the cached org list.
                        let previous_orgs = session_data
                            .user
                            .as_ref()
                            .map(|u| u.orgs.clone())
                            .unwrap_or_default();
                        let orgs = match state
                            .platform_client
                            .list_my_organisations(&session_data.access_token)
                            .await
                        {
                            Ok(fresh) => fresh
                                .into_iter()
                                .map(|o| CachedOrg {
                                    organisation_id: o.organisation_id,
                                    name: o.name,
                                    role: o.role,
                                })
                                .collect(),
                            Err(_) => previous_orgs,
                        };
                        session_data.user = Some(CachedUser {
                            user_id: user.user_id.clone(),
                            username: user.username.clone(),
                            profile_picture_url: user.profile_picture_url.clone(),
                            emails: user.emails,
                            orgs,
                        });
                    }

                    let _ = state.sessions.update(&session_id, session_data.clone()).await;
                }
                Err(_) => {
                    // Refresh token rejected - session is dead
                    let _ = state.sessions.delete(&session_id).await;
                    return Err(axum::response::Redirect::to("/login"));
                }
            }
        } else {
            // Refresh orgs if they're empty OR if the session hasn't been seen
            // for a while (e.g. after server restart, PG session loaded with stale orgs).
            let now = chrono::Utc::now();
            let orgs_empty = session_data
                .user
                .as_ref()
                .is_some_and(|u| u.orgs.is_empty());
            let orgs_stale = now - session_data.last_seen_at > chrono::Duration::minutes(5);
            let needs_org_refresh = orgs_empty || orgs_stale;

            if needs_org_refresh {
                if let Ok(orgs) = state
                    .platform_client
                    .list_my_organisations(&session_data.access_token)
                    .await
                {
                    if !orgs.is_empty() {
                        if let Some(ref mut user) = session_data.user {
                            tracing::info!(
                                user_id = %user.user_id,
                                org_count = orgs.len(),
                                was_empty = orgs_empty,
                                "refreshed org list"
                            );
                            user.orgs = orgs
                                .into_iter()
                                .map(|o| CachedOrg {
                                    organisation_id: o.organisation_id,
                                    name: o.name,
                                    role: o.role,
                                })
                                .collect();
                        }
                        session_data.last_seen_at = chrono::Utc::now();
                        let _ = state.sessions.update(&session_id, session_data.clone()).await;
                    }
                }
            } else {
                // Throttle last_seen_at writes: only update if older than 5 minutes
                let now = chrono::Utc::now();
                if now - session_data.last_seen_at > chrono::Duration::minutes(5) {
                    session_data.last_seen_at = now;
                    let _ = state.sessions.update(&session_id, session_data.clone()).await;
                }
            }
        }

        let user = session_data
            .user
            .clone()
            .ok_or(redirect)?;

        Ok(Session {
            session_id,
            access_token: session_data.access_token.clone(),
            user,
            csrf_token: session_data.csrf_token.clone(),
            needs_username: session_data.needs_username,
            session_data,
        })
    }
}

/// Extractor that optionally provides session info. Never rejects.
/// Used for pages that behave differently when authenticated (e.g., login/signup redirect).
pub struct MaybeSession {
    pub session: Option<Session>,
}

impl FromRequestParts<AppState> for MaybeSession {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state).await.ok();
        Ok(MaybeSession { session })
    }
}

/// Build a Set-Cookie header for the session.
/// When `remember` is true, the cookie persists for 30 days; otherwise it is a session cookie.
pub fn session_cookie(session_id: &SessionId, remember: bool) -> CookieJar {
    let mut builder = Cookie::build((SESSION_COOKIE, session_id.to_string()))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax);

    if remember {
        builder = builder.max_age(time::Duration::days(30));
    }

    CookieJar::new().add(builder.build())
}

/// Validate that a submitted CSRF token matches the session's token.
pub fn validate_csrf(session: &Session, submitted: &str) -> bool {
    !session.csrf_token.is_empty() && session.csrf_token == submitted
}

/// Build a Set-Cookie header that clears the session cookie.
pub fn clear_session_cookie() -> CookieJar {
    let mut cookie = Cookie::from(SESSION_COOKIE);
    cookie.set_path("/");
    cookie.make_removal();

    CookieJar::new().add(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_return_to_accepts_normal_paths() {
        assert_eq!(safe_return_to(Some("/device")), Some("/device"));
        assert_eq!(
            safe_return_to(Some("/device?user_code=ABCD-EFGH")),
            Some("/device?user_code=ABCD-EFGH")
        );
        assert_eq!(
            safe_return_to(Some("/orgs/foo/settings")),
            Some("/orgs/foo/settings")
        );
    }

    #[test]
    fn safe_return_to_rejects_unsafe_inputs() {
        assert_eq!(safe_return_to(None), None);
        assert_eq!(safe_return_to(Some("")), None);
        assert_eq!(safe_return_to(Some("/")), None);
        assert_eq!(safe_return_to(Some("device")), None);
        assert_eq!(safe_return_to(Some("https://evil.com")), None);
        assert_eq!(safe_return_to(Some("javascript:alert(1)")), None);
        assert_eq!(safe_return_to(Some("//evil.com/path")), None);
        assert_eq!(safe_return_to(Some("/\\evil.com")), None);
    }
}
