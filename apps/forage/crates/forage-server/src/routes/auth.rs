use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum::extract::Multipart;
use chrono::Utc;
use forage_core::integrations::SlackUserLink;
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error};
use crate::auth::{self, MaybeSession, Session};
use crate::state::AppState;
use axum_extra::extract::CookieJar;
use forage_core::auth::{
    validate_email, validate_password, validate_username, LoginResult, RegisterResult, UserEmail,
};
use forage_core::session::{CachedOrg, CachedUser, SessionData, generate_csrf_token};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", get(signup_page).post(signup_submit))
        .route("/login", get(login_page).post(login_submit))
        .route("/logout", post(logout_submit))
        .route(
            "/settings/tokens",
            get(tokens_page).post(create_token_submit),
        )
        .route("/settings/tokens/{id}/delete", post(delete_token_submit))
        .route("/settings/account", get(account_page))
        .route("/settings/account/username", post(update_username_submit))
        .route("/settings/account/password", post(change_password_submit))
        .route("/settings/account/emails", post(add_email_submit))
        .route(
            "/settings/account/emails/remove",
            post(remove_email_submit),
        )
        .route(
            "/settings/account/emails/resend-verification",
            post(resend_verification_submit),
        )
        .route(
            "/settings/account/notifications",
            post(update_notification_preference),
        )
        .route(
            "/settings/account/slack/connect",
            get(slack_connect),
        )
        .route(
            "/settings/account/slack/callback",
            get(slack_user_callback),
        )
        .route(
            "/settings/account/slack/disconnect",
            post(slack_disconnect),
        )
        .route(
            "/settings/account/github/connect",
            get(github_link_start),
        )
        .route(
            "/settings/account/github/disconnect",
            post(github_link_disconnect),
        )
        .route(
            "/settings/account/google/connect",
            get(google_link_start),
        )
        .route(
            "/settings/account/google/disconnect",
            post(google_link_disconnect),
        )
        .route("/settings/account/picture", post(upload_picture_submit))
        .route("/settings/account/picture/remove", post(remove_picture_submit))
        .route("/avatars/{user_id}", get(serve_avatar))
        .route("/auth/google", get(google_oauth_start))
        .route("/auth/google/callback", get(google_oauth_callback))
        .route("/auth/github", get(github_oauth_start))
        .route("/auth/github/callback", get(github_oauth_callback))
        .route("/auth/magic-link", get(magic_link_page).post(magic_link_request))
        .route("/auth/magic-link/verify", get(magic_link_verify))
        .route("/auth/verify-email", get(verify_email_redeem))
        .route(
            "/auth/verify-email/resend",
            get(verify_email_resend_page).post(verify_email_resend_submit),
        )
        .route(
            "/auth/complete-profile",
            get(complete_profile_page).post(complete_profile_submit),
        )
        .route("/login/mfa", post(login_mfa_submit))
        .route("/settings/account/mfa/setup", post(mfa_setup_start))
        .route("/settings/account/mfa/verify", post(mfa_verify_setup))
        .route("/settings/account/mfa/disable", post(mfa_disable))
}

// ─── Signup ─────────────────────────────────────────────────────────

async fn signup_page(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(params): Query<ReturnToParams>,
) -> Result<Response, axum::http::StatusCode> {
    let rt = auth::safe_return_to(params.return_to.as_deref());
    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    render_signup(&state, "", "", "", None, rt)
}

#[derive(Deserialize)]
struct SignupForm {
    username: String,
    email: String,
    password: String,
    password_confirm: String,
    #[serde(default)]
    return_to: Option<String>,
}

async fn signup_submit(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Form(form): Form<SignupForm>,
) -> Result<Response, axum::http::StatusCode> {
    let rt_owned = form.return_to.clone();
    let rt = auth::safe_return_to(rt_owned.as_deref());
    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    // Validate
    if let Err(e) = validate_username(&form.username) {
        return render_signup(&state, &form.username, &form.email, "", Some(e.0), rt);
    }
    if let Err(e) = validate_email(&form.email) {
        return render_signup(&state, &form.username, &form.email, "", Some(e.0), rt);
    }
    if let Err(e) = validate_password(&form.password) {
        return render_signup(&state, &form.username, &form.email, "", Some(e.0), rt);
    }
    if form.password != form.password_confirm {
        return render_signup(
            &state,
            &form.username,
            &form.email,
            "",
            Some("Passwords do not match".into()),
            rt,
        );
    }

    // Register via forest-server
    match state
        .forest_client
        .register(&form.username, &form.email, &form.password)
        .await
    {
        Ok(RegisterResult::VerificationRequired) => {
            // Forest withheld tokens. Drive the verification email and
            // show the "check your inbox" page. No session created.
            // Pass return_to so a device-login flow survives the
            // email-verify detour and gets back to /device after the
            // user signs in.
            if let Err(e) = enqueue_verification_email(&state, &form.email, rt).await {
                tracing::error!(error = %e, "failed to enqueue verification email after signup");
            }
            render_verify_email_check_inbox(&state, &form.email)
        }
        Ok(RegisterResult::Success(tokens)) => {
            // Fetch user info for the session cache
            let user_cache = match state
                .forest_client
                .get_user(&tokens.access_token)
                .await
            {
                Ok(u) => {
                    let orgs = match state
                        .platform_client
                        .list_my_organisations(&tokens.access_token)
                        .await
                    {
                        Ok(orgs) => orgs
                            .into_iter()
                            .map(|o| CachedOrg {
                                organisation_id: o.organisation_id,
                                name: o.name,
                                role: o.role,
                            })
                            .collect(),
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to fetch orgs during signup");
                            vec![]
                        }
                    };
                    Some(CachedUser {
                        user_id: u.user_id,
                        username: u.username,
                        profile_picture_url: u.profile_picture_url,
                        emails: u.emails,
                        orgs,
                    })
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to fetch user during signup");
                    None
                }
            };

            let now = Utc::now();
            let session_data = SessionData {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                access_expires_at: now + chrono::Duration::seconds(auth::cap_token_expiry(tokens.expires_in_seconds)),
                user: user_cache,
                csrf_token: generate_csrf_token(),
                created_at: now,
                last_seen_at: now,
                needs_username: false,
            };

            match state.sessions.create(session_data).await {
                Ok(session_id) => {
                    let cookie = auth::session_cookie(&session_id, true);
                    let dest = rt.unwrap_or("/dashboard");
                    Ok((cookie, Redirect::to(dest)).into_response())
                }
                Err(_) => render_signup(
                    &state,
                    &form.username,
                    &form.email,
                    "",
                    Some("Internal error. Please try again.".into()),
                    rt,
                ),
            }
        }
        Err(forage_core::auth::AuthError::AlreadyExists(_)) => render_signup(
            &state,
            &form.username,
            &form.email,
            "",
            Some("Username or email already registered".into()),
            rt,
        ),
        Err(forage_core::auth::AuthError::Unavailable(msg)) => {
            tracing::error!("forest-server unavailable: {msg}");
            render_signup(
                &state,
                &form.username,
                &form.email,
                "",
                Some("Service temporarily unavailable. Please try again.".into()),
                rt,
            )
        }
        Err(e) => render_signup(
            &state,
            &form.username,
            &form.email,
            "",
            Some(e.to_string()),
            rt,
        ),
    }
}

fn render_signup(
    state: &AppState,
    username: &str,
    email: &str,
    _password: &str,
    error: Option<String>,
    return_to: Option<&str>,
) -> Result<Response, axum::http::StatusCode> {
    let html = state
        .templates
        .render(
            "pages/signup.html.jinja",
            context! {
                title => "Sign Up - Forest",
                description => "Create your Forest account",
                is_auth_page => true,
                username => username,
                email => email,
                error => error,
                has_google_oauth => state.google_oauth_config.is_some(),
                has_github_oauth => state.github_oauth_config.is_some(),
                has_magic_link => state.magic_link_store.is_some(),
                return_to => return_to,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(html).into_response())
}

// ─── Login ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReturnToParams {
    return_to: Option<String>,
}

async fn login_page(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(params): Query<ReturnToParams>,
) -> Result<Response, axum::http::StatusCode> {
    let rt = auth::safe_return_to(params.return_to.as_deref());
    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    render_login(&state, "", None, rt)
}

#[derive(Deserialize)]
struct LoginForm {
    identifier: String,
    password: String,
    #[serde(default)]
    remember_me: Option<String>,
    #[serde(default)]
    return_to: Option<String>,
}

async fn login_submit(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Form(form): Form<LoginForm>,
) -> Result<Response, axum::http::StatusCode> {
    let return_to = form.return_to.clone();
    let rt = auth::safe_return_to(return_to.as_deref());

    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    if form.identifier.is_empty() || form.password.is_empty() {
        return render_login(
            &state,
            &form.identifier,
            Some("Email/username and password are required".into()),
            rt,
        );
    }

    match state
        .forest_client
        .login(&form.identifier, &form.password)
        .await
    {
        Ok(LoginResult::Success(tokens)) => {
            let user_cache = match state
                .forest_client
                .get_user(&tokens.access_token)
                .await
            {
                Ok(u) => {
                    let orgs = match state
                        .platform_client
                        .list_my_organisations(&tokens.access_token)
                        .await
                    {
                        Ok(orgs) => orgs
                            .into_iter()
                            .map(|o| CachedOrg {
                                organisation_id: o.organisation_id,
                                name: o.name,
                                role: o.role,
                            })
                            .collect(),
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to fetch orgs during login");
                            vec![]
                        }
                    };
                    Some(CachedUser {
                        user_id: u.user_id,
                        username: u.username,
                        profile_picture_url: u.profile_picture_url,
                        emails: u.emails,
                        orgs,
                    })
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to fetch user during login");
                    None
                }
            };

            let now = Utc::now();
            let session_data = SessionData {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                access_expires_at: now + chrono::Duration::seconds(auth::cap_token_expiry(tokens.expires_in_seconds)),
                user: user_cache,
                csrf_token: generate_csrf_token(),
                created_at: now,
                last_seen_at: now,
                needs_username: false,
            };

            let remember = form.remember_me.is_some();
            match state.sessions.create(session_data).await {
                Ok(session_id) => {
                    let cookie = auth::session_cookie(&session_id, remember);
                    let dest = rt.unwrap_or("/dashboard");
                    Ok((cookie, Redirect::to(dest)).into_response())
                }
                Err(_) => render_login(
                    &state,
                    &form.identifier,
                    Some("Internal error. Please try again.".into()),
                    rt,
                ),
            }
        }
        Ok(LoginResult::MfaRequired { mfa_session_token }) => {
            // Store the MFA session token in a short-lived cookie and show the challenge page.
            use axum::http::header::SET_COOKIE;
            use axum::http::HeaderValue;

            let cookie_value = format!(
                "forage_mfa_session={}; HttpOnly; SameSite=Lax; Path=/login; Max-Age=300",
                mfa_session_token
            );
            let html = state
                .templates
                .render(
                    "pages/mfa_challenge.html.jinja",
                    context! {
                        title => "Two-factor authentication - Forest",
                        description => "Enter your authenticator code to continue",
                        is_auth_page => true,
                        error => None::<String>,
                        return_to => rt,
                    },
                )
                .map_err(|e| {
                    tracing::error!("template error: {e:#}");
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                })?;

            let mut response = Html(html).into_response();
            if let Ok(val) = HeaderValue::from_str(&cookie_value) {
                response.headers_mut().insert(SET_COOKIE, val);
            }
            Ok(response)
        }
        Ok(LoginResult::EmailNotVerified) => {
            // Forest is enforcing email verification. Re-trigger a
            // verification email and show the resend page (idempotent
            // behavior if the user has already received one — rate
            // limit handles the rest).
            let email_for_resend = form.identifier.clone();
            if email_for_resend.contains('@') {
                if let Err(e) = enqueue_verification_email(&state, &email_for_resend, rt).await {
                    tracing::warn!(error = %e, "failed to enqueue verification email on login block");
                }
            }
            render_verify_email_check_inbox(&state, &email_for_resend)
        }
        Err(forage_core::auth::AuthError::InvalidCredentials) => render_login(
            &state,
            &form.identifier,
            Some("Invalid email/username or password".into()),
            rt,
        ),
        Err(forage_core::auth::AuthError::Unavailable(msg)) => {
            tracing::error!("forest-server unavailable: {msg}");
            render_login(
                &state,
                &form.identifier,
                Some("Service temporarily unavailable. Please try again.".into()),
                rt,
            )
        }
        Err(e) => render_login(&state, &form.identifier, Some(e.to_string()), rt),
    }
}

// ─── MFA challenge (login flow) ──────────────────────────────────────

#[derive(Deserialize)]
struct MfaForm {
    code: String,
    #[serde(default)]
    return_to: Option<String>,
}

async fn login_mfa_submit(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<MfaForm>,
) -> Result<Response, axum::http::StatusCode> {
    use axum::http::header::SET_COOKIE;
    use axum::http::HeaderValue;

    let rt = auth::safe_return_to(form.return_to.as_deref());

    let mfa_token = jar
        .get("forage_mfa_session")
        .map(|c| c.value().to_string())
        .ok_or_else(|| {
            tracing::warn!("MFA submit without mfa_session cookie");
            axum::http::StatusCode::BAD_REQUEST
        })?;

    let tokens = match state
        .forest_client
        .verify_login_mfa(&mfa_token, &form.code)
        .await
    {
        Ok(t) => t,
        Err(forage_core::auth::AuthError::InvalidCredentials) => {
            let html = state
                .templates
                .render(
                    "pages/mfa_challenge.html.jinja",
                    context! {
                        title => "Two-factor authentication - Forest",
                        description => "Enter your authenticator code to continue",
                        is_auth_page => true,
                        error => Some("Invalid or expired code. Please try again."),
                        return_to => rt,
                    },
                )
                .map_err(|e| {
                    tracing::error!("template error: {e:#}");
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                })?;
            return Ok(Html(html).into_response());
        }
        Err(e) => {
            tracing::error!("MFA verify error: {e}");
            let html = state
                .templates
                .render(
                    "pages/mfa_challenge.html.jinja",
                    context! {
                        title => "Two-factor authentication - Forest",
                        description => "Enter your authenticator code to continue",
                        is_auth_page => true,
                        error => Some(e.to_string()),
                        return_to => rt,
                    },
                )
                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
            return Ok(Html(html).into_response());
        }
    };

    let user_cache = match state.forest_client.get_user(&tokens.access_token).await {
        Ok(u) => {
            let orgs = state
                .platform_client
                .list_my_organisations(&tokens.access_token)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|o| CachedOrg {
                    organisation_id: o.organisation_id,
                    name: o.name,
                    role: o.role,
                })
                .collect();
            Some(CachedUser {
                user_id: u.user_id,
                username: u.username,
                profile_picture_url: u.profile_picture_url,
                emails: u.emails,
                orgs,
            })
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to fetch user after MFA");
            None
        }
    };

    let now = Utc::now();
    let session_data = SessionData {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        access_expires_at: now
            + chrono::Duration::seconds(auth::cap_token_expiry(tokens.expires_in_seconds)),
        user: user_cache,
        csrf_token: generate_csrf_token(),
        created_at: now,
        last_seen_at: now,
        needs_username: false,
    };

    match state.sessions.create(session_data).await {
        Ok(session_id) => {
            let session_cookie = auth::session_cookie(&session_id, true);
            // Clear the MFA session cookie
            let clear_mfa = "forage_mfa_session=; HttpOnly; SameSite=Lax; Path=/login; Max-Age=0";
            let dest = rt.unwrap_or("/dashboard");
            let mut response = (session_cookie, Redirect::to(dest)).into_response();
            if let Ok(val) = HeaderValue::from_str(clear_mfa) {
                response.headers_mut().append(SET_COOKIE, val);
            }
            Ok(response)
        }
        Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn render_login(
    state: &AppState,
    identifier: &str,
    error: Option<String>,
    return_to: Option<&str>,
) -> Result<Response, axum::http::StatusCode> {
    let html = state
        .templates
        .render(
            "pages/login.html.jinja",
            context! {
                title => "Sign In - Forest",
                description => "Sign in to your Forest account",
                is_auth_page => true,
                identifier => identifier,
                error => error,
                has_google_oauth => state.google_oauth_config.is_some(),
                has_github_oauth => state.github_oauth_config.is_some(),
                has_magic_link => state.magic_link_store.is_some(),
                return_to => return_to,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Html(html).into_response())
}

// ─── Logout ─────────────────────────────────────────────────────────

async fn logout_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfForm>,
) -> Result<impl IntoResponse, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(&state, StatusCode::FORBIDDEN, "Invalid request", "CSRF validation failed. Please try again."));
    }
    // Best-effort logout on forest-server
    if let Ok(Some(data)) = state.sessions.get(&session.session_id).await {
        let _ = state.forest_client.logout(&data.refresh_token).await;
    }
    let _ = state.sessions.delete(&session.session_id).await;
    Ok((auth::clear_session_cookie(), Redirect::to("/")))
}

// ─── Tokens ─────────────────────────────────────────────────────────

async fn tokens_page(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let tokens = state
        .forest_client
        .list_tokens(&session.access_token, &session.user.user_id)
        .await
        .unwrap_or_default();

    let html = state
        .templates
        .render(
            "pages/tokens.html.jinja",
            context! {
                title => "API Tokens - Forest",
                description => "Manage your personal access tokens",
                user => context! { username => session.user.username },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! { name => o.name, role => o.role }).collect::<Vec<_>>(),
                tokens => tokens.iter().map(|t| context! {
                    token_id => t.token_id,
                    name => t.name,
                    created_at => t.created_at,
                    last_used => t.last_used,
                    expires_at => t.expires_at,
                }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                created_token => None::<String>,
                active_tab => "tokens",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct CsrfForm {
    _csrf: String,
}

#[derive(Deserialize)]
struct CreateTokenForm {
    name: String,
    _csrf: String,
}

async fn create_token_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CreateTokenForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(&state, StatusCode::FORBIDDEN, "Invalid request", "CSRF validation failed. Please try again."));
    }

    let created = state
        .forest_client
        .create_token(&session.access_token, &session.user.user_id, &form.name)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to create token", &e)
        })?;

    let tokens = state
        .forest_client
        .list_tokens(&session.access_token, &session.user.user_id)
        .await
        .unwrap_or_default();

    let html = state
        .templates
        .render(
            "pages/tokens.html.jinja",
            context! {
                title => "API Tokens - Forest",
                description => "Manage your personal access tokens",
                user => context! { username => session.user.username },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! { name => o.name, role => o.role }).collect::<Vec<_>>(),
                tokens => tokens.iter().map(|t| context! {
                    token_id => t.token_id,
                    name => t.name,
                    created_at => t.created_at,
                    last_used => t.last_used,
                    expires_at => t.expires_at,
                }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                created_token => Some(created.raw_token),
                active_tab => "tokens",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

async fn delete_token_submit(
    State(state): State<AppState>,
    session: Session,
    axum::extract::Path(token_id): axum::extract::Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(&state, StatusCode::FORBIDDEN, "Invalid request", "CSRF validation failed. Please try again."));
    }

    state
        .forest_client
        .delete_token(&session.access_token, &token_id)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to delete token", &e)
        })?;

    Ok(Redirect::to("/settings/tokens").into_response())
}

// ─── Account settings ────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct AccountPageQuery {
    flash: Option<String>,
    error: Option<String>,
}

async fn account_page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<AccountPageQuery>,
) -> Result<Response, Response> {
    let prefs = state
        .platform_client
        .get_notification_preferences(&session.access_token)
        .await
        .unwrap_or_default();

    let slack_links = if let Some(store) = state.integration_store.as_ref() {
        store
            .list_slack_user_links(&session.user.user_id)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };

    let forest_identities = state
        .forest_client
        .list_linked_identities(&session.access_token, &session.user.user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to fetch linked identities");
            vec![]
        });

    let linked_accounts =
        forage_core::auth::merge_linked_identities(forest_identities, &slack_links);

    // Fetch fresh user info to get current mfa_enabled state.
    let mfa_enabled = state
        .forest_client
        .get_user(&session.access_token)
        .await
        .map(|u| u.mfa_enabled)
        .unwrap_or(false);

    render_account(
        &state,
        &session,
        None,
        &prefs,
        &slack_links,
        &linked_accounts,
        mfa_enabled,
        flash_message(query.flash.as_deref()),
        error_message(query.error.as_deref()),
    )
}

/// Translate a `?flash=...` query parameter into a user-facing message.
/// Returning `None` keeps the banner hidden.
fn flash_message(flash: Option<&str>) -> Option<&'static str> {
    match flash? {
        "linked_github" => Some("GitHub account linked."),
        "linked_google" => Some("Google account linked."),
        "verification_resent" => Some("Verification email sent. Check your inbox."),
        _ => None,
    }
}

/// Translate a `?error=...` query parameter into a user-facing message.
fn error_message(error: Option<&str>) -> Option<&'static str> {
    match error? {
        "access_denied_github" => Some("GitHub authorisation was cancelled."),
        "access_denied_google" => Some("Google authorisation was cancelled."),
        "already_linked_other_github" => {
            Some("This GitHub account is already linked to another Forest user.")
        }
        "already_linked_other_google" => {
            Some("This Google account is already linked to another Forest user.")
        }
        "already_linked_github" => {
            Some("You already have a GitHub account linked. Disconnect it first to switch.")
        }
        "already_linked_google" => {
            Some("You already have a Google account linked. Disconnect it first to switch.")
        }
        "link_failed_github" => Some("Linking your GitHub account failed. Please try again."),
        "link_failed_google" => Some("Linking your Google account failed. Please try again."),
        "last_auth_method" => Some(
            "You can't disconnect your only sign-in method. \
             Set a password or link another provider first.",
        ),
        "verification_resend_ineligible" => {
            Some("That email isn't eligible for verification right now.")
        }
        "verification_resend_failed" => {
            Some("Couldn't send the verification email. Please try again.")
        }
        _ => None,
    }
}

#[allow(clippy::result_large_err)]
#[allow(clippy::too_many_arguments)]
fn render_account(
    state: &AppState,
    session: &Session,
    error: Option<&str>,
    notification_prefs: &[forage_core::platform::NotificationPreference],
    slack_links: &[SlackUserLink],
    linked_accounts: &[forage_core::auth::LinkedIdentity],
    mfa_enabled: bool,
    flash: Option<&str>,
    oauth_error: Option<&str>,
) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/account.html.jinja",
            context! {
                title => "Account Settings - Forest",
                description => "Manage your account settings",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                    profile_picture_url => &session.user.profile_picture_url,
                    mfa_enabled => mfa_enabled,
                    emails => session.user.emails.iter().map(|e| context! {
                        email => &e.email,
                        verified => e.verified,
                    }).collect::<Vec<_>>(),
                },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! { name => o.name, role => o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                error => error,
                active_tab => "account",
                enabled_prefs => notification_prefs.iter()
                    .filter(|p| p.enabled)
                    .map(|p| format!("{}|{}", p.notification_type, p.channel))
                    .collect::<Vec<_>>(),
                has_slack_oauth => state.slack_config.is_some(),
                has_github_oauth => state.github_oauth_config.is_some(),
                has_google_oauth => state.google_oauth_config.is_some(),
                slack_links => slack_links.iter().map(|l| context! {
                    id => &l.id,
                    team_id => &l.team_id,
                    team_name => &l.team_name,
                    slack_user_id => &l.slack_user_id,
                    slack_username => &l.slack_username,
                }).collect::<Vec<_>>(),
                linked_accounts => linked_accounts.iter().map(|l| context! {
                    provider => l.provider.as_str(),
                    provider_display => l.provider.display_name(),
                    external_id => &l.external_id,
                    display_name => &l.display_name,
                    email => &l.email,
                    avatar_url => &l.avatar_url,
                    subtitle => &l.subtitle,
                    linked_at => &l.linked_at,
                    disconnect_key => &l.disconnect_key,
                }).collect::<Vec<_>>(),
                has_github_link => linked_accounts.iter().any(|l| l.provider == forage_core::auth::LinkedProvider::GitHub),
                has_google_link => linked_accounts.iter().any(|l| l.provider == forage_core::auth::LinkedProvider::Google),
                flash => flash,
                oauth_error => oauth_error,
            },
        )
        .map_err(|e| {
            internal_error(state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct UpdateUsernameForm {
    username: String,
    _csrf: String,
}

async fn update_username_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<UpdateUsernameForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    if let Err(e) = validate_username(&form.username) {
        return render_account(&state, &session, Some(&e.0), &[], &[], &[], false, None, None);
    }

    match state
        .forest_client
        .update_username(&session.access_token, &session.user.user_id, &form.username)
        .await
    {
        Ok(updated_user) => {
            // Update cached username in session
            if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
                if let Some(ref mut user) = session_data.user {
                    user.username = updated_user.username;
                }
                let _ = state
                    .sessions
                    .update(&session.session_id, session_data)
                    .await;
            }
            Ok(Redirect::to("/settings/account").into_response())
        }
        Err(forage_core::auth::AuthError::AlreadyExists(_)) => {
            render_account(&state, &session, Some("Username is already taken."), &[], &[], &[], false, None, None)
        }
        Err(e) => {
            tracing::error!("failed to update username: {e}");
            render_account(&state, &session, Some("Could not update username. Please try again."), &[], &[], &[], false, None, None)
        }
    }
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    current_password: String,
    new_password: String,
    new_password_confirm: String,
    _csrf: String,
}

async fn change_password_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    if form.new_password != form.new_password_confirm {
        return render_account(&state, &session, Some("New passwords do not match."), &[], &[], &[], false, None, None);
    }

    if let Err(e) = validate_password(&form.new_password) {
        return render_account(&state, &session, Some(&e.0), &[], &[], &[], false, None, None);
    }

    match state
        .forest_client
        .change_password(
            &session.access_token,
            &session.user.user_id,
            &form.current_password,
            &form.new_password,
        )
        .await
    {
        Ok(()) => Ok(Redirect::to("/settings/account").into_response()),
        Err(forage_core::auth::AuthError::InvalidCredentials) => {
            render_account(&state, &session, Some("Current password is incorrect."), &[], &[], &[], false, None, None)
        }
        Err(e) => {
            tracing::error!("failed to change password: {e}");
            render_account(&state, &session, Some("Could not change password. Please try again."), &[], &[], &[], false, None, None)
        }
    }
}

#[derive(Deserialize)]
struct AddEmailForm {
    email: String,
    _csrf: String,
}

async fn add_email_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<AddEmailForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    if let Err(e) = validate_email(&form.email) {
        return render_account(&state, &session, Some(&e.0), &[], &[], &[], false, None, None);
    }

    match state
        .forest_client
        .add_email(&session.access_token, &session.user.user_id, &form.email)
        .await
    {
        Ok(result) => {
            // Update cached emails in session
            if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
                if let Some(ref mut user) = session_data.user {
                    user.emails.push(UserEmail {
                        email: result.email.email.clone(),
                        verified: result.email.verified,
                    });
                }
                let _ = state
                    .sessions
                    .update(&session.session_id, session_data)
                    .await;
            }
            // Drive the verification flow for the newly added email if
            // forest signaled it.
            if result.email_verification_required {
                if let Err(e) =
                    enqueue_verification_email(&state, &result.email.email, None).await
                {
                    tracing::warn!(error = %e, "failed to enqueue verification email on add_email");
                }
            }
            Ok(Redirect::to("/settings/account").into_response())
        }
        Err(forage_core::auth::AuthError::AlreadyExists(_)) => {
            render_account(&state, &session, Some("Email is already registered."), &[], &[], &[], false, None, None)
        }
        Err(e) => {
            tracing::error!("failed to add email: {e}");
            render_account(&state, &session, Some("Could not add email. Please try again."), &[], &[], &[], false, None, None)
        }
    }
}

#[derive(Deserialize)]
struct RemoveEmailForm {
    email: String,
    _csrf: String,
}

async fn remove_email_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RemoveEmailForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    match state
        .forest_client
        .remove_email(&session.access_token, &session.user.user_id, &form.email)
        .await
    {
        Ok(()) => {
            // Update cached emails in session
            if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
                if let Some(ref mut user) = session_data.user {
                    user.emails.retain(|e| e.email != form.email);
                }
                let _ = state
                    .sessions
                    .update(&session.session_id, session_data)
                    .await;
            }
            Ok(Redirect::to("/settings/account").into_response())
        }
        Err(e) => {
            tracing::error!("failed to remove email: {e}");
            render_account(&state, &session, Some("Could not remove email. Please try again."), &[], &[], &[], false, None, None)
        }
    }
}

// ─── Resend verification email (account settings) ───────────────────

#[derive(Deserialize)]
struct ResendVerificationForm {
    _csrf: String,
    email: String,
}

/// Resend a verification email for an unverified address on the
/// session user's account. Triggered by the "Try sending again" button
/// next to each unverified email in /settings/account.
///
/// Ownership check is critical: we only resend for emails that already
/// belong to the caller — otherwise this becomes a free "trigger an
/// email to anyone" endpoint and forest's email-verification token
/// (issued by forage) leaks signal about who has an account.
async fn resend_verification_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ResendVerificationForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    // The submitted email must belong to the caller AND must currently
    // be unverified — verified emails don't need a resend, and a
    // resend request for one that's not on the account is a likely
    // probe / tampered form. The error message is generic to avoid
    // leaking which case triggered (not-yours vs already-verified).
    let owned_unverified = session
        .user
        .emails
        .iter()
        .any(|e| e.email == form.email && !e.verified);
    if !owned_unverified {
        return Ok(Redirect::to(
            "/settings/account?error=verification_resend_ineligible",
        )
        .into_response());
    }

    if let Err(e) = enqueue_verification_email(&state, &form.email, None).await {
        tracing::warn!(
            error = %e,
            user_id = %session.user.user_id,
            "resend verification email failed to enqueue"
        );
        return Ok(Redirect::to(
            "/settings/account?error=verification_resend_failed",
        )
        .into_response());
    }

    tracing::info!(
        user_id = %session.user.user_id,
        "resend verification email enqueued from account page"
    );
    Ok(Redirect::to("/settings/account?flash=verification_resent").into_response())
}

// ─── MFA setup / disable (account settings) ──────────────────────────

async fn mfa_setup_start(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    let setup = state
        .forest_client
        .setup_mfa(&session.access_token, &session.user.user_id)
        .await
        .map_err(|e| internal_error(&state, "failed to begin MFA setup", &e))?;

    let qr_svg = generate_qr_svg(&setup.provisioning_uri);

    let html = state
        .templates
        .render(
            "pages/mfa_setup.html.jinja",
            context! {
                title => "Set up two-factor authentication - Forest",
                description => "Scan the QR code with your authenticator app",
                user => context! { username => &session.user.username },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! { name => o.name, role => o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                mfa_id => &setup.mfa_id,
                provisioning_uri => &setup.provisioning_uri,
                secret => &setup.secret,
                qr_svg => qr_svg,
                active_tab => "account",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct MfaVerifySetupForm {
    mfa_id: String,
    code: String,
    _csrf: String,
}

async fn mfa_verify_setup(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<MfaVerifySetupForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    state
        .forest_client
        .verify_mfa_setup(&session.access_token, &form.mfa_id, &form.code)
        .await
        .map_err(|e| {
            // Re-render the setup page with an error rather than a bare 500
            let msg = match &e {
                forage_core::auth::AuthError::InvalidCredentials => {
                    "Invalid code. Please check your authenticator app and try again.".to_string()
                }
                other => other.to_string(),
            };
            error_page(&state, StatusCode::BAD_REQUEST, "Verification failed", &msg)
        })?;

    Ok(Redirect::to("/settings/account").into_response())
}

#[derive(Deserialize)]
struct MfaDisableForm {
    code: String,
    _csrf: String,
}

async fn mfa_disable(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<MfaDisableForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    state
        .forest_client
        .disable_mfa(&session.access_token, &session.user.user_id, &form.code)
        .await
        .map_err(|e| {
            let msg = match &e {
                forage_core::auth::AuthError::InvalidCredentials => {
                    "Invalid code. Please enter your current authenticator code.".to_string()
                }
                other => other.to_string(),
            };
            error_page(&state, StatusCode::BAD_REQUEST, "Could not disable MFA", &msg)
        })?;

    Ok(Redirect::to("/settings/account").into_response())
}

/// Generate a simple SVG QR code from a provisioning URI using the qrcode crate.
fn generate_qr_svg(data: &str) -> String {
    use qrcode::{QrCode, EcLevel};
    use qrcode::render::svg;

    match QrCode::with_error_correction_level(data, EcLevel::M) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

// ─── Notification preferences ────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateNotificationPreferenceForm {
    _csrf: String,
    notification_type: String,
    channel: String,
    enabled: String,
}

async fn update_notification_preference(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<UpdateNotificationPreferenceForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Invalid CSRF token.",
        ));
    }

    let enabled = form.enabled == "true";

    state
        .platform_client
        .set_notification_preference(
            &session.access_token,
            &form.notification_type,
            &form.channel,
            enabled,
        )
        .await
        .map_err(|e| internal_error(&state, "set notification preference", &e))?;

    Ok(Redirect::to("/settings/account").into_response())
}

// ─── Slack user enrollment ────────────────────────────────────────────

async fn slack_connect(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, Response> {
    let slack_config = state.slack_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Slack not configured",
            "Slack OAuth is not configured on this server.",
        )
    })?;

    let redirect_uri = format!(
        "{}/settings/account/slack/callback",
        slack_config.redirect_host
    );
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&user_scope=identity.basic&redirect_uri={}&state={}",
        urlencoding::encode(&slack_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&session.user.user_id),
    );

    Ok(Redirect::to(&url))
}

#[derive(Deserialize)]
struct SlackUserCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn slack_user_callback(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<SlackUserCallbackQuery>,
) -> Result<Response, Response> {
    // Handle user-denied case
    if let Some(err) = query.error {
        tracing::warn!("Slack user OAuth denied: {err}");
        return Ok(Redirect::to("/settings/account").into_response());
    }

    let code = query.code.ok_or_else(|| {
        error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Missing authorization code from Slack.",
        )
    })?;

    // Verify state matches our user_id to prevent CSRF
    let state_param = query.state.unwrap_or_default();
    if state_param != session.user.user_id {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "State parameter mismatch. Please try connecting again.",
        ));
    }

    let slack_config = state.slack_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not configured",
            "Slack OAuth is not configured.",
        )
    })?;

    let integration_store = state.integration_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Slack account linking requires a database. Set DATABASE_URL to enable.",
        )
    })?;

    let redirect_uri = format!(
        "{}/settings/account/slack/callback",
        slack_config.redirect_host
    );

    // Exchange the authorization code for a user access token
    let http = reqwest::Client::new();
    let token_resp = http
        .post("https://slack.com/api/oauth.v2.access")
        .form(&[
            ("client_id", slack_config.client_id.as_str()),
            ("client_secret", slack_config.client_secret.as_str()),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| {
            internal_error(
                &state,
                "slack user oauth",
                &format!("Failed to contact Slack: {e}"),
            )
        })?;

    let resp_body: serde_json::Value = token_resp.json().await.map_err(|e| {
        internal_error(
            &state,
            "slack user oauth",
            &format!("Failed to parse Slack response: {e}"),
        )
    })?;

    if resp_body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err_msg = resp_body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        tracing::error!("Slack user OAuth error: {err_msg}");
        return Err(error_page(
            &state,
            StatusCode::BAD_GATEWAY,
            "Slack error",
            &format!("Slack returned an error: {err_msg}"),
        ));
    }

    // For user-scoped OAuth, the user token is nested under authed_user
    let authed_user = resp_body.get("authed_user").ok_or_else(|| {
        internal_error(
            &state,
            "slack user oauth",
            &"Missing authed_user in Slack response",
        )
    })?;

    let slack_user_id = authed_user["id"].as_str().unwrap_or("").to_string();
    let user_access_token = authed_user["access_token"].as_str().unwrap_or("").to_string();
    let team_id = resp_body["team"]["id"].as_str().unwrap_or("").to_string();
    let team_name = resp_body["team"]["name"].as_str().unwrap_or("").to_string();

    // Fetch display name via users.identity (requires identity.basic user scope)
    let slack_username = if user_access_token.is_empty() {
        slack_user_id.clone()
    } else {
        let identity_name: Option<String> = async {
            let r = http
                .get("https://slack.com/api/users.identity")
                .bearer_auth(&user_access_token)
                .send()
                .await
                .ok()?;
            let body: serde_json::Value = r.json().await.ok()?;
            let name = body["user"]["name"].as_str()?.to_string();
            if name.is_empty() { None } else { Some(name) }
        }
        .await;

        identity_name.unwrap_or_else(|| slack_user_id.clone())
    };

    let now = chrono::Utc::now().to_rfc3339();
    let link = SlackUserLink {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: session.user.user_id.clone(),
        team_id,
        team_name,
        slack_user_id,
        slack_username,
        created_at: now,
    };

    integration_store
        .upsert_slack_user_link(&link)
        .await
        .map_err(|e| internal_error(&state, "upsert slack user link", &e))?;

    Ok(Redirect::to("/settings/account").into_response())
}

#[derive(Deserialize)]
struct SlackDisconnectForm {
    team_id: String,
    _csrf: String,
}

async fn slack_disconnect(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SlackDisconnectForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    let integration_store = state.integration_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Slack account linking requires a database.",
        )
    })?;

    integration_store
        .delete_slack_user_link(&session.user.user_id, &form.team_id)
        .await
        .map_err(|e| internal_error(&state, "delete slack user link", &e))?;

    Ok(Redirect::to("/settings/account").into_response())
}

// ─── Google OAuth ───────────────────────────────────────────────────

use axum::http::HeaderMap;

/// Generate a random state value for OAuth CSRF protection (same entropy as CSRF tokens).
fn generate_oauth_state() -> String {
    generate_csrf_token()
}

/// Pick the post-auth destination for a freshly-minted session.
///
/// New (OAuth/magic-link) users must pick a username via
/// `/auth/complete-profile`; we forward `return_to` through that step so
/// the original intent survives. Returning users go straight to
/// `return_to` (validated) or `/dashboard` as fallback.
fn post_auth_dest(is_new_user: bool, return_to: Option<&str>) -> String {
    if is_new_user {
        match return_to {
            Some(rt) => format!(
                "/auth/complete-profile?return_to={}",
                urlencoding::encode(rt)
            ),
            None => "/auth/complete-profile".to_string(),
        }
    } else {
        return_to.unwrap_or("/dashboard").to_string()
    }
}

/// Lifetime for OAuth/MFA flow state rows. Matches the cookie Max-Age
/// used by the state-anchor cookies (10 min).
const OAUTH_FLOW_TTL_SECS: i64 = 600;

/// Write a row to the OAuth state store, if one is configured. No-op
/// without a store (the cookie-based CSRF anchor still works; we just
/// lose the per-flow `return_to`).
async fn persist_oauth_flow(
    state: &AppState,
    provider: &str,
    oauth_state: &str,
    return_to: Option<&str>,
) -> Result<(), Response> {
    let Some(store) = state.oauth_state_store.as_ref() else {
        return Ok(());
    };
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(OAUTH_FLOW_TTL_SECS);
    store
        .create(provider, oauth_state, return_to, expires_at)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, provider, "failed to persist OAuth flow state");
            error_page(
                state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Login unavailable",
                "Could not start the sign-in flow. Please try again.",
            )
        })
}

/// Read-and-delete the OAuth flow row. Returns the previously-stored
/// `return_to` (validated via [`auth::safe_return_to`]) or `None` if no
/// store is wired, the row is missing/expired, or the value isn't safe.
async fn consume_oauth_flow_return_to(
    state: &AppState,
    provider: &str,
    oauth_state: &str,
) -> Option<String> {
    let store = state.oauth_state_store.as_ref()?;
    match store.consume(provider, oauth_state).await {
        Ok(Some(flow)) => flow
            .return_to
            .filter(|r| auth::safe_return_to(Some(r)).is_some()),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, provider, "OAuth flow state consume failed");
            None
        }
    }
}

/// GET /auth/google — redirect to Google's consent screen.
async fn google_oauth_start(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(rt_params): Query<ReturnToParams>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(rt_params.return_to.as_deref());
    if maybe.session.is_some() {
        // Logged-in user hit the bare login route. Clear any stale link
        // cookie *before* bouncing — otherwise an abandoned link flow's
        // cookie lingers until expiry and could mis-dispatch a later
        // login callback.
        let dest = rt.unwrap_or("/dashboard");
        return Ok(redirect_clearing_link_cookie(dest, "google"));
    }

    let config = state.google_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Google sign-in is not configured.",
        )
    })?;

    let oauth_state = generate_oauth_state();
    let redirect_uri = format!("{}/auth/google/callback", config.redirect_host);

    // Persist per-flow return_to keyed by the OAuth state. The cookie
    // below anchors the flow to this browser; the store is what keeps
    // return_to safe from cross-tab cookie overwrite.
    persist_oauth_flow(
        &state,
        forage_core::auth::oauth_state::PROVIDER_GOOGLE,
        &oauth_state,
        rt,
    )
    .await?;

    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=offline",
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(&oauth_state),
    );

    // Store state in a short-lived cookie for validation on callback.
    let state_cookie = format!(
        "forage_oauth_state={}; HttpOnly; SameSite=Lax; Path=/auth/google; Max-Age=600",
        oauth_state
    );
    // Clear any stale link-purpose cookie left from an abandoned link
    // flow on this browser — otherwise the callback would dispatch into
    // the link branch unexpectedly.
    let clear_link = "forage_oauth_link_user=; HttpOnly; SameSite=Lax; Path=/auth/google; Max-Age=0";

    let mut headers = HeaderMap::new();
    headers.append(
        axum::http::header::SET_COOKIE,
        state_cookie.parse().unwrap(),
    );
    headers.append(
        axum::http::header::SET_COOKIE,
        clear_link.parse().unwrap(),
    );
    headers.insert(
        axum::http::header::LOCATION,
        auth_url.parse().unwrap(),
    );

    Ok((StatusCode::FOUND, headers).into_response())
}

/// OAuth 2.0 authorization-code callback query parameters. Used by
/// both Google and GitHub callbacks — both providers conform to the
/// same RFC 6749 §4.1.2 shape (`code` on success, `error` on denial,
/// always `state`). Renamed from `GoogleCallbackQuery` so the GitHub
/// callback doesn't pick up Google-specific fields if this struct ever
/// gains one.
#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// GET /auth/google/callback — handle the OAuth callback from Google.
///
/// See `github_oauth_callback` doc for the link-vs-login dispatch logic.
async fn google_oauth_callback(
    State(state): State<AppState>,
    maybe: MaybeSession,
    jar: CookieJar,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Response, Response> {
    // ── 1. CSRF anchor: validate the OAuth state cookie FIRST. ────────
    // Doing this before reading the link cookie prevents an attacker
    // from leaking link-vs-login dispatch context through error-path
    // disclosure (e.g. a forged `?error=access_denied` with a planted
    // link cookie used to bounce to `/settings/account` rather than
    // `/login`). State mismatch produces a generic 403 — no clue about
    // which branch we'd have taken.
    let expected_state = jar
        .get("forage_oauth_state")
        .map(|c| c.value().to_string());
    let received_state = query.state.as_deref().unwrap_or("");
    // Require a non-empty expected state so an injected empty-value
    // `forage_oauth_state=` cookie combined with a missing `?state=`
    // parameter (both `""`) can't pass the equality check.
    match expected_state {
        Some(ref expected) if !expected.is_empty() && expected == received_state => {}
        _ => {
            return Err(error_page(
                &state,
                StatusCode::FORBIDDEN,
                "Invalid request",
                "OAuth state mismatch. Please try again.",
            ));
        }
    }

    // Read-and-delete the per-flow return_to keyed by the OAuth state.
    // Single-use — must happen at most once per callback irrespective of
    // which branch we take below.
    let return_to = consume_oauth_flow_return_to(
        &state,
        forage_core::auth::oauth_state::PROVIDER_GOOGLE,
        received_state,
    )
    .await;

    // ── 2. Dispatch link-vs-login. ────────────────────────────────────
    let link_cookie_user_id = jar
        .get(LINK_PURPOSE_COOKIE)
        .map(|c| c.value().to_string());
    let is_link_flow = link_cookie_user_id.is_some()
        && maybe
            .session
            .as_ref()
            .map(|s| s.user.user_id.clone())
            == link_cookie_user_id;

    if !is_link_flow && link_cookie_user_id.is_some() {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Link mismatch",
            "Your session does not match the account that started this link. Please try again.",
        ));
    }

    if maybe.session.is_some() && !is_link_flow {
        let dest = return_to.as_deref().unwrap_or("/dashboard");
        return Ok(Redirect::to(dest).into_response());
    }

    // Handle denial from Google.
    if query.error.is_some() {
        return Ok(Redirect::to(if is_link_flow {
            "/settings/account?error=access_denied_google"
        } else {
            "/login"
        })
        .into_response());
    }

    let config = state.google_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Google sign-in is not configured.",
        )
    })?;

    let code = query.code.as_deref().unwrap_or("");
    if code.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Missing authorization code.",
        ));
    }

    let redirect_uri = format!("{}/auth/google/callback", config.redirect_host);

    // Step 1: Exchange the authorization code with Google via OIDC.
    let oidc = state.google_oidc_exchange.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Google sign-in is not configured.",
        )
    })?;

    let identity = oidc
        .exchange_code(code, &redirect_uri)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Google OIDC exchange failed");
            error_page(
                &state,
                StatusCode::BAD_GATEWAY,
                "Login failed",
                "Failed to verify your Google account. Please try again.",
            )
        })?;

    // If we're linking (not logging in), branch into the link flow.
    if is_link_flow {
        let session = maybe.session.as_ref().expect("is_link_flow implies session");
        return Ok(complete_link_flow(
            &state,
            session,
            "google",
            forage_core::auth::LinkedProvider::Google,
            identity,
        )
        .await);
    }

    // Step 2: Tell Forest to find-or-create this user by their verified identity.
    let result = state
        .forest_client
        .oauth_login("google", &identity.sub, &identity.email, &identity.name, identity.picture_url.as_deref())
        .await
        .map_err(|e| match &e {
            forage_core::auth::AuthError::Unavailable(_) => error_page(
                &state,
                StatusCode::SERVICE_UNAVAILABLE,
                "Temporarily unavailable",
                "Authentication service is temporarily unavailable. Please try again later.",
            ),
            _ => {
                tracing::error!(error = %e, "Google OAuth login failed");
                error_page(
                    &state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Login failed",
                    "Google sign-in failed. Please try again.",
                )
            }
        })?;

    // Build session (same pattern as login_submit / signup_submit).
    let user_cache = {
        let orgs = match state
            .platform_client
            .list_my_organisations(&result.tokens.access_token)
            .await
        {
            Ok(orgs) => orgs
                .into_iter()
                .map(|o| CachedOrg {
                    organisation_id: o.organisation_id,
                    name: o.name,
                    role: o.role,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch orgs during OAuth login");
                vec![]
            }
        };
        Some(CachedUser {
            user_id: result.user.user_id,
            username: result.user.username,
            profile_picture_url: result.user.profile_picture_url,
            emails: result.user.emails,
            orgs,
        })
    };

    let now = Utc::now();
    let session_data = SessionData {
        access_token: result.tokens.access_token,
        refresh_token: result.tokens.refresh_token,
        access_expires_at: now
            + chrono::Duration::seconds(auth::cap_token_expiry(
                result.tokens.expires_in_seconds,
            )),
        user: user_cache,
        csrf_token: generate_csrf_token(),
        created_at: now,
        last_seen_at: now,
        needs_username: result.is_new_user,
    };

    let session_id = state.sessions.create(session_data).await.map_err(|e| {
        tracing::error!(error = %e, "failed to create session");
        error_page(
            &state,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Login failed",
            "Internal error. Please try again.",
        )
    })?;

    let mut jar = auth::session_cookie(&session_id, true);

    // Clear the OAuth state cookie.
    let mut clear_cookie = axum_extra::extract::cookie::Cookie::from("forage_oauth_state");
    clear_cookie.set_path("/auth/google");
    clear_cookie.make_removal();
    jar = jar.add(clear_cookie);

    let redirect_to = post_auth_dest(result.is_new_user, return_to.as_deref());
    Ok((jar, Redirect::to(&redirect_to)).into_response())
}

// ─── GitHub OAuth ───────────────────────────────────────────────────

/// GET /auth/github — redirect to GitHub's authorization screen.
async fn github_oauth_start(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(rt_params): Query<ReturnToParams>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(rt_params.return_to.as_deref());
    if maybe.session.is_some() {
        let dest = rt.unwrap_or("/dashboard");
        return Ok(redirect_clearing_link_cookie(dest, "github"));
    }

    let config = state.github_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "GitHub sign-in is not configured.",
        )
    })?;

    let oauth_state = generate_oauth_state();
    let redirect_uri = format!("{}/auth/github/callback", config.redirect_host);

    persist_oauth_flow(
        &state,
        forage_core::auth::oauth_state::PROVIDER_GITHUB,
        &oauth_state,
        rt,
    )
    .await?;

    let auth_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("read:user user:email"),
        urlencoding::encode(&oauth_state),
    );

    let state_cookie = format!(
        "forage_oauth_state={}; HttpOnly; SameSite=Lax; Path=/auth/github; Max-Age=600",
        oauth_state
    );
    // Clear any stale link-purpose cookie (see google_oauth_start).
    let clear_link = "forage_oauth_link_user=; HttpOnly; SameSite=Lax; Path=/auth/github; Max-Age=0";

    let mut headers = HeaderMap::new();
    headers.append(
        axum::http::header::SET_COOKIE,
        state_cookie.parse().unwrap(),
    );
    headers.append(
        axum::http::header::SET_COOKIE,
        clear_link.parse().unwrap(),
    );
    headers.insert(
        axum::http::header::LOCATION,
        auth_url.parse().unwrap(),
    );

    Ok((StatusCode::FOUND, headers).into_response())
}

/// GET /auth/github/callback — handle the OAuth callback from GitHub.
///
/// Dispatches between two flows based on the `forage_oauth_link_user`
/// cookie set by `/settings/account/github/connect`:
///   - Cookie present + session matches → **link flow** (Forest
///     `LinkOAuthProvider`).
///   - Cookie absent + no session → **login flow** (existing
///     `OAuthLogin` behaviour).
///   - Cookie absent + session present → bounce to /dashboard.
///   - Cookie present + session missing or mismatched → 403.
async fn github_oauth_callback(
    State(state): State<AppState>,
    maybe: MaybeSession,
    jar: CookieJar,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Response, Response> {
    // CSRF anchor first — see google_oauth_callback for rationale.
    let expected_state = jar
        .get("forage_oauth_state")
        .map(|c| c.value().to_string());
    let received_state = query.state.as_deref().unwrap_or("");
    // Require a non-empty expected state so an injected empty-value
    // `forage_oauth_state=` cookie combined with a missing `?state=`
    // parameter (both `""`) can't pass the equality check.
    match expected_state {
        Some(ref expected) if !expected.is_empty() && expected == received_state => {}
        _ => {
            return Err(error_page(
                &state,
                StatusCode::FORBIDDEN,
                "Invalid request",
                "OAuth state mismatch. Please try again.",
            ));
        }
    }

    // Single-use read of the per-flow return_to (see google_oauth_callback).
    let return_to = consume_oauth_flow_return_to(
        &state,
        forage_core::auth::oauth_state::PROVIDER_GITHUB,
        received_state,
    )
    .await;

    let link_cookie_user_id = jar
        .get(LINK_PURPOSE_COOKIE)
        .map(|c| c.value().to_string());
    let is_link_flow = link_cookie_user_id.is_some()
        && maybe
            .session
            .as_ref()
            .map(|s| s.user.user_id.clone())
            == link_cookie_user_id;

    if !is_link_flow && link_cookie_user_id.is_some() {
        // Cookie set for linking, but session is missing or belongs to
        // a different user — refuse to proceed.
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Link mismatch",
            "Your session does not match the account that started this link. Please try again.",
        ));
    }

    if maybe.session.is_some() && !is_link_flow {
        let dest = return_to.as_deref().unwrap_or("/dashboard");
        return Ok(Redirect::to(dest).into_response());
    }

    if query.error.is_some() {
        return Ok(Redirect::to(if is_link_flow {
            "/settings/account?error=access_denied_github"
        } else {
            "/login"
        })
        .into_response());
    }

    let config = state.github_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "GitHub sign-in is not configured.",
        )
    })?;

    let code = query.code.as_deref().unwrap_or("");
    if code.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Missing authorization code.",
        ));
    }

    let redirect_uri = format!("{}/auth/github/callback", config.redirect_host);

    // Step 1: Exchange code with GitHub via OIDC exchange.
    let oidc = state.github_oidc_exchange.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "GitHub sign-in is not configured.",
        )
    })?;

    let identity = oidc
        .exchange_code(code, &redirect_uri)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "GitHub OIDC exchange failed");
            error_page(
                &state,
                StatusCode::BAD_GATEWAY,
                "Login failed",
                "Failed to verify your GitHub account. Please try again.",
            )
        })?;

    // If we're linking (not logging in), branch into the link flow.
    if is_link_flow {
        let session = maybe.session.as_ref().expect("is_link_flow implies session");
        return Ok(complete_link_flow(
            &state,
            session,
            "github",
            forage_core::auth::LinkedProvider::GitHub,
            identity,
        )
        .await);
    }

    // Step 2: Tell Forest to find-or-create this user.
    let result = state
        .forest_client
        .oauth_login("github", &identity.sub, &identity.email, &identity.name, identity.picture_url.as_deref())
        .await
        .map_err(|e| match &e {
            forage_core::auth::AuthError::Unavailable(_) => error_page(
                &state,
                StatusCode::SERVICE_UNAVAILABLE,
                "Temporarily unavailable",
                "Authentication service is temporarily unavailable. Please try again later.",
            ),
            _ => {
                tracing::error!(error = %e, "GitHub OAuth login failed");
                error_page(
                    &state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Login failed",
                    "GitHub sign-in failed. Please try again.",
                )
            }
        })?;

    // Build session (same pattern as Google).
    let user_cache = {
        let orgs = match state
            .platform_client
            .list_my_organisations(&result.tokens.access_token)
            .await
        {
            Ok(orgs) => orgs
                .into_iter()
                .map(|o| CachedOrg {
                    organisation_id: o.organisation_id,
                    name: o.name,
                    role: o.role,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch orgs during GitHub login");
                vec![]
            }
        };
        Some(CachedUser {
            user_id: result.user.user_id,
            username: result.user.username,
            profile_picture_url: result.user.profile_picture_url,
            emails: result.user.emails,
            orgs,
        })
    };

    let now = Utc::now();
    let session_data = SessionData {
        access_token: result.tokens.access_token,
        refresh_token: result.tokens.refresh_token,
        access_expires_at: now
            + chrono::Duration::seconds(auth::cap_token_expiry(
                result.tokens.expires_in_seconds,
            )),
        user: user_cache,
        csrf_token: generate_csrf_token(),
        created_at: now,
        last_seen_at: now,
        needs_username: result.is_new_user,
    };

    let session_id = state.sessions.create(session_data).await.map_err(|e| {
        tracing::error!(error = %e, "failed to create session");
        error_page(
            &state,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Login failed",
            "Internal error. Please try again.",
        )
    })?;

    let mut jar = auth::session_cookie(&session_id, true);
    let mut clear_cookie = axum_extra::extract::cookie::Cookie::from("forage_oauth_state");
    clear_cookie.set_path("/auth/github");
    clear_cookie.make_removal();
    jar = jar.add(clear_cookie);

    let redirect_to = post_auth_dest(result.is_new_user, return_to.as_deref());
    Ok((jar, Redirect::to(&redirect_to)).into_response())
}

// ─── Complete Profile (username selection for OAuth users) ──────────

async fn complete_profile_page(
    State(state): State<AppState>,
    session: Session,
    Query(params): Query<ReturnToParams>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(params.return_to.as_deref());
    if !session.needs_username {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    render_complete_profile(&state, &session.csrf_token, "", None, rt)
}

#[derive(Deserialize)]
struct CompleteProfileForm {
    _csrf: String,
    username: String,
    #[serde(default)]
    return_to: Option<String>,
}

async fn complete_profile_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CompleteProfileForm>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(form.return_to.as_deref());
    if !session.needs_username {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    // Validate CSRF.
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF token mismatch.",
        ));
    }

    // Validate username.
    if let Err(e) = validate_username(&form.username) {
        return render_complete_profile(&state, &session.csrf_token, &form.username, Some(e.0), rt);
    }

    // Update username on forest-server.
    state
        .forest_client
        .update_username(&session.access_token, &session.user.user_id, &form.username)
        .await
        .map_err(|e| match &e {
            forage_core::auth::AuthError::AlreadyExists(_) => {
                render_complete_profile(
                    &state,
                    &session.csrf_token,
                    &form.username,
                    Some("Username is already taken.".into()),
                    rt,
                )
                .unwrap_err()
            }
            _ => {
                tracing::error!(error = %e, "failed to update username");
                error_page(
                    &state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Update failed",
                    "Failed to set username. Please try again.",
                )
            }
        })?;

    // Update session: set new username and clear needs_username flag.
    let mut updated = session.session_data.clone();
    if let Some(ref mut user) = updated.user {
        user.username = form.username;
    }
    updated.needs_username = false;
    state
        .sessions
        .update(&session.session_id, updated)
        .await
        .map_err(|e| internal_error(&state, "update session after username", &e))?;

    let dest = rt.unwrap_or("/dashboard");
    Ok(Redirect::to(dest).into_response())
}

fn render_complete_profile(
    state: &AppState,
    csrf_token: &str,
    username: &str,
    error: Option<String>,
    return_to: Option<&str>,
) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/complete_profile.html.jinja",
            context! {
                title => "Choose Username - Forest",
                description => "Pick a username for your new account",
                is_auth_page => true,
                csrf_token => csrf_token,
                username => username,
                error => error,
                return_to => return_to,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            error_page(
                state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "Template rendering failed.",
            )
        })?;

    Ok(Html(html).into_response())
}

// ─── Magic Link ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MagicLinkForm {
    email: String,
    #[serde(default)]
    return_to: Option<String>,
}

/// GET /auth/magic-link — show the magic link email form.
async fn magic_link_page(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(rt_params): Query<ReturnToParams>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(rt_params.return_to.as_deref());
    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    if state.magic_link_store.is_none() {
        return Err(error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Magic link login is not configured.",
        ));
    }

    render_magic_link_form(&state, "", None, rt)
}

/// POST /auth/magic-link — send a magic link email.
async fn magic_link_request(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Form(form): Form<MagicLinkForm>,
) -> Result<Response, Response> {
    let rt = auth::safe_return_to(form.return_to.as_deref());
    if maybe.session.is_some() {
        return Ok(Redirect::to(rt.unwrap_or("/dashboard")).into_response());
    }

    let store = state.magic_link_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Magic link login is not configured.",
        )
    })?;

    // Validate email.
    if let Err(e) = validate_email(&form.email) {
        return render_magic_link_form(&state, &form.email, Some(e.0), rt);
    }

    // Rate limit: max 3 per 15 minutes per email.
    let since = Utc::now() - chrono::Duration::minutes(15);
    let count = store
        .count_recent(
            forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK,
            &form.email,
            since,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "magic link count_recent failed");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error. Please try again.",
            )
        })?;
    if count >= 3 {
        return render_magic_link_sent(&state, &form.email);
    }

    // Generate token and store hash.
    let (raw_token, token_hash) =
        forage_core::auth::magic_link::generate_magic_link_token();
    let expires_at = Utc::now() + chrono::Duration::minutes(15);

    store
        .store_token(
            forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK,
            &token_hash,
            &form.email,
            expires_at,
            rt,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "magic link store_token failed");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error. Please try again.",
            )
        })?;

    // Build magic link URL.
    let verify_url = format!(
        "{}/auth/magic-link/verify?token={}",
        state.forage_host,
        urlencoding::encode(&raw_token)
    );

    // Publish email job to NATS (if available).
    if let Some(ref js) = state.email_jetstream {
        let envelope = forage_core::integrations::email::EmailEnvelope {
            to: form.email.clone(),
            subject: "Sign in to Forest".into(),
            body_html: format!(
                "<p>Click the link below to sign in to Forest:</p>\
                 <p><a href=\"{verify_url}\">Sign in to Forest</a></p>\
                 <p>This link expires in 15 minutes.</p>\
                 <p>If you didn't request this, you can safely ignore this email.</p>"
            ),
            body_text: format!(
                "Sign in to Forest\n\n\
                 Click this link to sign in: {verify_url}\n\n\
                 This link expires in 15 minutes.\n\
                 If you didn't request this, you can safely ignore this email."
            ),
            email_type: "magic-link".into(),
        };

        let payload = serde_json::to_vec(&envelope).map_err(|e| {
            tracing::error!(error = %e, "failed to serialize email envelope");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error.",
            )
        })?;

        let subject = forage_core::integrations::email::email_subject("magic-link");
        if let Err(e) = js.publish(subject, payload.into()).await {
            tracing::error!(error = %e, "failed to publish magic link email to NATS");
        }
    } else {
        tracing::warn!(email = %form.email, url = %verify_url, "NATS not configured, magic link email not sent (token stored)");
    }

    // Always show success (prevents email enumeration).
    render_magic_link_sent(&state, &form.email)
}

#[derive(Deserialize)]
struct MagicLinkVerifyQuery {
    token: Option<String>,
}

/// GET /auth/magic-link/verify?token=xxx — verify and consume a magic link token.
async fn magic_link_verify(
    State(state): State<AppState>,
    maybe: MaybeSession,
    Query(query): Query<MagicLinkVerifyQuery>,
) -> Result<Response, Response> {
    if maybe.session.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }

    let store = state.magic_link_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Magic link login is not configured.",
        )
    })?;

    let raw_token = query.token.as_deref().unwrap_or("");
    if raw_token.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid link",
            "Missing token.",
        ));
    }

    let token_hash = forage_core::auth::magic_link::hash_magic_link_token(raw_token);

    let consumed = store
        .verify_and_consume(
            forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK,
            &token_hash,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "magic link verify_and_consume failed");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error. Please try again.",
            )
        })?
        .ok_or_else(|| {
            error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "Link expired",
                "This magic link has expired or has already been used. Please request a new one.",
            )
        })?;

    let email = consumed.email;
    let return_to = consumed
        .return_to
        .filter(|r| auth::safe_return_to(Some(r)).is_some());

    // Call Forest to find-or-create user by email identity.
    let result = state
        .forest_client
        .oauth_login("magic-link", &email, &email, "", None)
        .await
        .map_err(|e| match &e {
            forage_core::auth::AuthError::Unavailable(_) => error_page(
                &state,
                StatusCode::SERVICE_UNAVAILABLE,
                "Temporarily unavailable",
                "Authentication service is temporarily unavailable. Please try again later.",
            ),
            _ => {
                tracing::error!(error = %e, "magic link login failed");
                error_page(
                    &state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Login failed",
                    "Magic link login failed. Please try again.",
                )
            }
        })?;

    // Build session (same pattern as Google OAuth callback).
    let user_cache = {
        let orgs = match state
            .platform_client
            .list_my_organisations(&result.tokens.access_token)
            .await
        {
            Ok(orgs) => orgs
                .into_iter()
                .map(|o| CachedOrg {
                    organisation_id: o.organisation_id,
                    name: o.name,
                    role: o.role,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch orgs during magic link login");
                vec![]
            }
        };
        Some(CachedUser {
            user_id: result.user.user_id,
            username: result.user.username,
            profile_picture_url: result.user.profile_picture_url,
            emails: result.user.emails,
            orgs,
        })
    };

    let now = Utc::now();
    let session_data = SessionData {
        access_token: result.tokens.access_token,
        refresh_token: result.tokens.refresh_token,
        access_expires_at: now
            + chrono::Duration::seconds(auth::cap_token_expiry(
                result.tokens.expires_in_seconds,
            )),
        user: user_cache,
        csrf_token: generate_csrf_token(),
        created_at: now,
        last_seen_at: now,
        needs_username: result.is_new_user,
    };

    let session_id = state.sessions.create(session_data).await.map_err(|e| {
        tracing::error!(error = %e, "failed to create session");
        error_page(
            &state,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Login failed",
            "Internal error. Please try again.",
        )
    })?;

    let jar = auth::session_cookie(&session_id, true);

    let redirect_to = post_auth_dest(result.is_new_user, return_to.as_deref());

    Ok((jar, Redirect::to(&redirect_to)).into_response())
}

fn render_magic_link_form(
    state: &AppState,
    email: &str,
    error: Option<String>,
    return_to: Option<&str>,
) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/magic_link.html.jinja",
            context! {
                title => "Sign in with Email - Forest",
                description => "Get a magic link sent to your email",
                email => email,
                error => error,
                return_to => return_to,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            error_page(
                state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "Template rendering failed.",
            )
        })?;

    Ok(Html(html).into_response())
}

fn render_magic_link_sent(state: &AppState, email: &str) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/magic_link_sent.html.jinja",
            context! {
                title => "Check Your Email - Forest",
                description => "Magic link sent",
                email => email,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            error_page(
                state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "Template rendering failed.",
            )
        })?;

    Ok(Html(html).into_response())
}

// ─── Profile Picture ─────────────────────────────────────────────────

const MAX_PICTURE_SIZE: usize = 2 * 1024 * 1024; // 2 MB
const MAX_AVATAR_DIMENSION: u32 = 512;

/// Decode an uploaded image and re-encode as WebP.
/// This strips any embedded payloads (polyglot files, EXIF scripts, etc.)
/// and normalizes the output to a safe, known format.
fn reencode_avatar(data: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(data)
        .map_err(|e| format!("failed to decode image: {e}"))?;

    // Resize if larger than max dimension, preserving aspect ratio.
    let img = if img.width() > MAX_AVATAR_DIMENSION || img.height() > MAX_AVATAR_DIMENSION {
        img.resize(
            MAX_AVATAR_DIMENSION,
            MAX_AVATAR_DIMENSION,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::WebP)
        .map_err(|e| format!("failed to encode as WebP: {e}"))?;
    Ok(buf.into_inner())
}

async fn upload_picture_submit(
    State(state): State<AppState>,
    session: Session,
    mut multipart: Multipart,
) -> Result<Response, Response> {
    let store = state.profile_picture_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Profile picture uploads are not available.",
        )
    })?;

    let mut file_data: Option<Vec<u8>> = None;
    let mut csrf_token: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "_csrf" => {
                csrf_token = field.text().await.ok();
            }
            "picture" => {
                let data = field.bytes().await.map_err(|e| {
                    tracing::error!("failed to read upload: {e}");
                    error_page(
                        &state,
                        StatusCode::BAD_REQUEST,
                        "Upload failed",
                        "Failed to read the uploaded file.",
                    )
                })?;

                if data.len() > MAX_PICTURE_SIZE {
                    return Err(error_page(
                        &state,
                        StatusCode::BAD_REQUEST,
                        "File too large",
                        "Profile picture must be under 2 MB.",
                    ));
                }

                if data.is_empty() {
                    continue;
                }

                file_data = Some(data.to_vec());
            }
            _ => {}
        }
    }

    if !auth::validate_csrf(&session, csrf_token.as_deref().unwrap_or("")) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    let raw_data = file_data.ok_or_else(|| {
        error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "No file",
            "Please select a file to upload.",
        )
    })?;

    // Decode and re-encode as WebP to strip any embedded payloads.
    let webp_data = reencode_avatar(&raw_data).map_err(|e| {
        tracing::warn!("image re-encode failed: {e}");
        error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid image",
            "Could not process the uploaded image. Please use a JPEG, PNG, or WebP file.",
        )
    })?;

    store
        .upsert(&session.user.user_id, "image/webp", &webp_data)
        .await
        .map_err(|e| internal_error(&state, "failed to store profile picture", &e))?;

    let avatar_url = format!("{}/avatars/{}", state.forage_host, session.user.user_id);
    state
        .forest_client
        .update_profile_picture_url(
            &session.access_token,
            &session.user.user_id,
            Some(&avatar_url),
        )
        .await
        .map_err(|e| {
            tracing::error!("failed to update profile picture URL in forest: {e}");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Update failed",
                "Failed to update your profile picture. Please try again.",
            )
        })?;

    // Update session cache with new picture URL.
    if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
        if let Some(ref mut user) = session_data.user {
            user.profile_picture_url = Some(avatar_url);
        }
        let _ = state.sessions.update(&session.session_id, session_data).await;
    }

    Ok(Redirect::to("/settings/account").into_response())
}

async fn remove_picture_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    if let Some(store) = state.profile_picture_store.as_ref() {
        store
            .delete(&session.user.user_id)
            .await
            .map_err(|e| internal_error(&state, "failed to delete profile picture", &e))?;
    }

    state
        .forest_client
        .update_profile_picture_url(&session.access_token, &session.user.user_id, Some(""))
        .await
        .map_err(|e| {
            tracing::error!("failed to clear profile picture URL: {e}");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Update failed",
                "Failed to remove your profile picture. Please try again.",
            )
        })?;

    // Update session cache to clear picture URL.
    if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
        if let Some(ref mut user) = session_data.user {
            user.profile_picture_url = None;
        }
        let _ = state.sessions.update(&session.session_id, session_data).await;
    }

    Ok(Redirect::to("/settings/account").into_response())
}

async fn serve_avatar(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Response {
    let store = match state.profile_picture_store.as_ref() {
        Some(s) => s,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    match store.get(&user_id).await {
        Ok(Some(pic)) => {
            let headers = [
                (axum::http::header::CONTENT_TYPE, pic.content_type),
                (
                    axum::http::header::CACHE_CONTROL,
                    "public, max-age=3600".to_string(),
                ),
                (
                    axum::http::header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; style-src 'unsafe-inline'".to_string(),
                ),
                (
                    axum::http::header::X_CONTENT_TYPE_OPTIONS,
                    "nosniff".to_string(),
                ),
            ];
            (headers, pic.data).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

// ─── Email verification (signup + add_email) ─────────────────────────

async fn enqueue_verification_email(
    state: &AppState,
    email: &str,
    return_to: Option<&str>,
) -> anyhow::Result<()> {
    let store = state
        .magic_link_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("magic-link store not configured"))?;

    // Rate-limit: 3 sends per email per 15 minutes (per type).
    let since = Utc::now() - chrono::Duration::minutes(15);
    let count = store
        .count_recent(
            forage_core::auth::magic_link::TOKEN_TYPE_EMAIL_VERIFY,
            email,
            since,
        )
        .await?;
    if count >= 3 {
        tracing::info!(email = %email, "verification email rate-limited (>=3 in last 15 min)");
        return Ok(());
    }

    let (raw_token, token_hash) = forage_core::auth::magic_link::generate_magic_link_token();
    let expires_at = Utc::now() + chrono::Duration::minutes(15);

    store
        .store_token(
            forage_core::auth::magic_link::TOKEN_TYPE_EMAIL_VERIFY,
            &token_hash,
            email,
            expires_at,
            return_to,
        )
        .await?;

    let verify_url = format!(
        "{}/auth/verify-email?token={}",
        state.forage_host,
        urlencoding::encode(&raw_token)
    );

    if let Some(ref js) = state.email_jetstream {
        let envelope = forage_core::integrations::email::EmailEnvelope {
            to: email.to_string(),
            subject: "Verify your email for Forest".into(),
            body_html: format!(
                "<p>Click the link below to verify your email for Forest:</p>\
                 <p><a href=\"{verify_url}\">Verify your email</a></p>\
                 <p>This link expires in 15 minutes.</p>\
                 <p>If you didn't request this, you can safely ignore this email.</p>"
            ),
            body_text: format!(
                "Verify your email for Forest\n\n\
                 Click this link to verify your email: {verify_url}\n\n\
                 This link expires in 15 minutes.\n\
                 If you didn't request this, you can safely ignore this email."
            ),
            email_type: forage_core::auth::magic_link::TOKEN_TYPE_EMAIL_VERIFY.into(),
        };

        let payload = serde_json::to_vec(&envelope)?;
        let subject = forage_core::integrations::email::email_subject(
            forage_core::auth::magic_link::TOKEN_TYPE_EMAIL_VERIFY,
        );
        if let Err(e) = js.publish(subject, payload.into()).await {
            tracing::error!(error = %e, "failed to publish verification email to NATS");
        }
    } else {
        tracing::warn!(
            email = %email,
            url = %verify_url,
            "NATS not configured, verification email not sent (token stored)",
        );
    }

    Ok(())
}

fn render_verify_email_check_inbox(
    state: &AppState,
    email: &str,
) -> Result<Response, axum::http::StatusCode> {
    let html = state
        .templates
        .render(
            "pages/verify_email_check_inbox.html.jinja",
            context! {
                title => "Verify your email - Forest",
                description => "Check your inbox for a verification link",
                email => email,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct VerifyEmailQuery {
    token: Option<String>,
}

async fn verify_email_redeem(
    State(state): State<AppState>,
    Query(query): Query<VerifyEmailQuery>,
) -> Result<Response, Response> {
    let store = state.magic_link_store.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Verification unavailable",
            "Email verification is not configured on this Forest instance.",
        )
    })?;

    let raw_token = query.token.as_deref().unwrap_or("");
    if raw_token.is_empty() {
        return Ok(render_verify_email_failed(&state));
    }

    let token_hash = forage_core::auth::magic_link::hash_magic_link_token(raw_token);

    let consumed = store
        .verify_and_consume(
            forage_core::auth::magic_link::TOKEN_TYPE_EMAIL_VERIFY,
            &token_hash,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "verify_and_consume failed for email-verify token");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error. Please try again.",
            )
        })?;

    let Some((email, stored_return_to)) = consumed.map(|c| (c.email, c.return_to)) else {
        return Ok(render_verify_email_failed(&state));
    };
    // Validate the stored value at consume time too (defence in depth —
    // the row could outlive a policy change to `safe_return_to`).
    let return_to = stored_return_to
        .as_deref()
        .and_then(|r| auth::safe_return_to(Some(r)))
        .map(|r| r.to_string());

    if let Err(e) = state.forest_client.confirm_email_verification(&email).await {
        tracing::error!(error = %e, email = %email, "confirm_email_verification failed");
        // Token is already consumed (single-use absolute). User must
        // request a new one via /auth/verify-email/resend.
        return Ok(render_verify_email_failed(&state));
    }

    // If a return_to was carried from the original signup/login attempt,
    // smooth out the journey: send the (still-unauthenticated) user to
    // /login with that return_to preserved, so post-login they land on
    // the pending intent (e.g. /device?user_code=…). Without this, the
    // user lands on a success page and has to re-discover their entry
    // point manually — that's the workaround DATA-251 calls out.
    if let Some(rt) = return_to {
        return Ok(Redirect::to(&format!(
            "/login?return_to={}",
            urlencoding::encode(&rt)
        ))
        .into_response());
    }

    let html = state
        .templates
        .render(
            "pages/verify_email_success.html.jinja",
            context! {
                title => "Email verified - Forest",
                description => "Your email has been verified",
                email => email,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error.",
            )
        })?;

    // Set Referrer-Policy: no-referrer so the raw token never leaks
    // via a Referer header on links inside the success page.
    let mut response = Html(html).into_response();
    response.headers_mut().insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

fn render_verify_email_failed(state: &AppState) -> Response {
    match state.templates.render(
        "pages/verify_email_failed.html.jinja",
        context! {
            title => "Verification link expired - Forest",
            description => "Verification link expired or already used",
        },
    ) {
        Ok(html) => {
            let mut resp = Html(html).into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
        Err(e) => internal_error(state, "verify_email_failed template", &e),
    }
}

#[derive(Deserialize)]
struct VerifyEmailResendForm {
    email: String,
}

async fn verify_email_resend_page(State(state): State<AppState>) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/verify_email_resend.html.jinja",
            context! {
                title => "Resend verification email - Forest",
                description => "Send a new verification link",
                email => "",
                error => None::<String>,
            },
        )
        .map_err(|e| {
            tracing::error!("template error: {e:#}");
            error_page(
                &state,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error",
                "Internal error.",
            )
        })?;
    Ok(Html(html).into_response())
}

async fn verify_email_resend_submit(
    State(state): State<AppState>,
    Form(form): Form<VerifyEmailResendForm>,
) -> Result<Response, Response> {
    // Always render the "check your inbox" page regardless of validation
    // outcome to avoid leaking which emails are registered. Internal
    // logs capture the actual outcome.
    if validate_email(&form.email).is_err() {
        tracing::info!(email = %form.email, "verify-email resend skipped: invalid email");
        return render_verify_email_check_inbox(&state, &form.email).map_err(|s| s.into_response());
    }
    if let Err(e) = enqueue_verification_email(&state, &form.email, None).await {
        tracing::warn!(error = %e, "verify-email resend enqueue failed");
    }
    render_verify_email_check_inbox(&state, &form.email).map_err(|s| s.into_response())
}

// ─── Account-level OAuth account linking (GitHub / Google) ──────────
//
// These routes let a signed-in user explicitly link an external OAuth
// identity to their Forest account. Distinct from `/auth/<provider>`,
// which mints a session. The two share the provider's authorize URL
// and callback; dispatch happens via the `forage_oauth_link_user`
// cookie set here. See `specs/features/010-account-integrations.md`.

const LINK_PURPOSE_COOKIE: &str = "forage_oauth_link_user";

#[allow(clippy::result_large_err)]
fn build_link_start_response(
    state: &AppState,
    session: &Session,
    provider: &str,
    authorize_url: &str,
    state_token: &str,
) -> Result<Response, Response> {
    let cookie_path = format!("/auth/{provider}");
    let state_cookie = format!(
        "forage_oauth_state={state_token}; HttpOnly; SameSite=Lax; Path={cookie_path}; Max-Age=600"
    );
    let link_cookie = format!(
        "{LINK_PURPOSE_COOKIE}={user_id}; HttpOnly; SameSite=Lax; Path={cookie_path}; Max-Age=600",
        user_id = session.user.user_id,
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.append(axum::http::header::SET_COOKIE, state_cookie.parse().map_err(|_| {
        internal_error(state, "cookie parse", &"state cookie")
    })?);
    headers.append(axum::http::header::SET_COOKIE, link_cookie.parse().map_err(|_| {
        internal_error(state, "cookie parse", &"link cookie")
    })?);
    headers.insert(axum::http::header::LOCATION, authorize_url.parse().map_err(|_| {
        internal_error(state, "url parse", &authorize_url)
    })?);

    Ok((StatusCode::FOUND, headers).into_response())
}

/// Build the cookie header values that clear the link/state cookies for a
/// provider — used after a successful or failed link callback so the
/// cookies don't leak into a later login attempt.
fn clear_link_cookies(provider: &str) -> [String; 2] {
    let path = format!("/auth/{provider}");
    [
        format!("forage_oauth_state=; HttpOnly; SameSite=Lax; Path={path}; Max-Age=0"),
        format!("{LINK_PURPOSE_COOKIE}=; HttpOnly; SameSite=Lax; Path={path}; Max-Age=0"),
    ]
}

/// Build a 302 redirect response that *also* clears the link-purpose
/// cookie for the given provider. Used by the login-start routes when
/// they bounce an already-authenticated user — without this, an
/// abandoned link cookie would survive the redirect and mis-dispatch a
/// later login callback.
fn redirect_clearing_link_cookie(location: &str, provider: &str) -> Response {
    let clear = format!(
        "{LINK_PURPOSE_COOKIE}=; HttpOnly; SameSite=Lax; Path=/auth/{provider}; Max-Age=0"
    );
    let mut headers = axum::http::HeaderMap::new();
    if let Ok(v) = clear.parse() {
        headers.append(axum::http::header::SET_COOKIE, v);
    }
    if let Ok(v) = location.parse() {
        headers.insert(axum::http::header::LOCATION, v);
    }
    (StatusCode::FOUND, headers).into_response()
}

/// GET /settings/account/github/connect — kick off the GitHub linking
/// flow. Requires an active session; sets the link-purpose cookie and
/// redirects to GitHub's authorize URL. The callback at
/// `/auth/github/callback` dispatches based on this cookie.
async fn github_link_start(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let config = state.github_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "GitHub sign-in is not configured.",
        )
    })?;

    let oauth_state = generate_oauth_state();
    let redirect_uri = format!("{}/auth/github/callback", config.redirect_host);
    let auth_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("read:user user:email"),
        urlencoding::encode(&oauth_state),
    );

    build_link_start_response(&state, &session, "github", &auth_url, &oauth_state)
}

/// GET /settings/account/google/connect — symmetric to the GitHub
/// version. Uses `openid email profile` scope.
async fn google_link_start(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let config = state.google_oauth_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Google sign-in is not configured.",
        )
    })?;

    let oauth_state = generate_oauth_state();
    let redirect_uri = format!("{}/auth/google/callback", config.redirect_host);
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&access_type=offline",
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode("openid email profile"),
        urlencoding::encode(&oauth_state),
    );

    build_link_start_response(&state, &session, "google", &auth_url, &oauth_state)
}

#[derive(Deserialize)]
struct ProviderDisconnectForm {
    _csrf: String,
}

/// POST /settings/account/github/disconnect — unlink the user's GitHub
/// identity from their Forest account.
async fn github_link_disconnect(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ProviderDisconnectForm>,
) -> Result<Response, Response> {
    disconnect_oauth_provider(
        &state,
        &session,
        &form._csrf,
        forage_core::auth::LinkedProvider::GitHub,
    )
    .await
}

/// POST /settings/account/google/disconnect — unlink the user's Google
/// identity from their Forest account.
async fn google_link_disconnect(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ProviderDisconnectForm>,
) -> Result<Response, Response> {
    disconnect_oauth_provider(
        &state,
        &session,
        &form._csrf,
        forage_core::auth::LinkedProvider::Google,
    )
    .await
}

async fn disconnect_oauth_provider(
    state: &AppState,
    session: &Session,
    csrf: &str,
    provider: forage_core::auth::LinkedProvider,
) -> Result<Response, Response> {
    if !auth::validate_csrf(session, csrf) {
        return Err(error_page(
            state,
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Invalid CSRF token.",
        ));
    }

    state
        .forest_client
        .unlink_oauth_provider(&session.access_token, &session.user.user_id, provider)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, provider = provider.as_str(), "unlink oauth provider failed");
            match &e {
                forage_core::auth::AuthError::PermissionDenied(_) => error_page(
                    state,
                    StatusCode::FORBIDDEN,
                    "Forbidden",
                    "You are not allowed to unlink this account.",
                ),
                forage_core::auth::AuthError::NotAuthenticated
                | forage_core::auth::AuthError::InvalidCredentials => Redirect::to("/login").into_response(),
                // Forest refused to strip the last sign-in method. Send
                // the user back to the account page with a banner —
                // `error_message()` renders the human-readable string.
                forage_core::auth::AuthError::LastAuthMethod => {
                    Redirect::to("/settings/account?error=last_auth_method").into_response()
                }
                _ => error_page(
                    state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Unlink failed",
                    "Failed to unlink the account. Please try again.",
                ),
            }
        })?;

    Ok(Redirect::to("/settings/account").into_response())
}

/// Continue the OAuth callback as a *link* flow (rather than login).
/// Reused by both github and google callbacks once they have decoded
/// the OIDC identity.
async fn complete_link_flow(
    state: &AppState,
    session: &Session,
    provider_str: &str,
    provider: forage_core::auth::LinkedProvider,
    identity: forage_core::auth::OidcIdentity,
) -> Response {
    let input = forage_core::auth::link_input_from_oidc(provider, &identity);
    let clear = clear_link_cookies(provider_str);

    let result = state
        .forest_client
        .link_oauth_provider(&session.access_token, &session.user.user_id, &input)
        .await;

    let mut headers = axum::http::HeaderMap::new();
    for c in &clear {
        if let Ok(v) = c.parse() {
            headers.append(axum::http::header::SET_COOKIE, v);
        }
    }

    let location: String = match result {
        Ok(()) => format!("/settings/account?flash=linked_{provider_str}"),
        Err(forage_core::auth::AuthError::AlreadyExists(msg))
            if msg.contains("already linked to another user") =>
        {
            format!("/settings/account?error=already_linked_other_{provider_str}")
        }
        Err(forage_core::auth::AuthError::AlreadyExists(_)) => {
            format!("/settings/account?error=already_linked_{provider_str}")
        }
        Err(e) => {
            tracing::error!(error = %e, provider = provider_str, "link oauth provider failed");
            format!("/settings/account?error=link_failed_{provider_str}")
        }
    };

    headers.insert(
        axum::http::header::LOCATION,
        match location.parse() {
            Ok(v) => v,
            Err(_) => "/settings/account".parse().unwrap(),
        },
    );

    (StatusCode::FOUND, headers).into_response()
}

