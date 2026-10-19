use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::Cookie;

use forage_core::session::{CachedOrg, CachedUser, SessionId};

use crate::state::AppState;

pub const SESSION_COOKIE: &str = "forage_session";

const MAX_TOKEN_LIFETIME_SECS: i64 = 86400;

pub fn cap_token_expiry(expires_in_seconds: i64) -> i64 {
    expires_in_seconds.min(MAX_TOKEN_LIFETIME_SECS)
}

pub struct Session {
    pub session_id: SessionId,
    pub access_token: String,
    pub user: CachedUser,
    pub csrf_token: String,
    pub needs_username: bool,
    pub session_data: forage_core::session::SessionData,
}

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

pub(crate) fn login_redirect(uri: &axum::http::Uri) -> axum::response::Redirect {
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

                    if let Ok(user) = state
                        .forest_client
                        .get_user(&session_data.access_token)
                        .await
                    {
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
                    let _ = state.sessions.delete(&session_id).await;
                    return Err(axum::response::Redirect::to("/login"));
                }
            }
        } else {
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

pub fn validate_csrf(session: &Session, submitted: &str) -> bool {
    !session.csrf_token.is_empty() && session.csrf_token == submitted
}

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
