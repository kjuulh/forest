use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use forage_core::integrations::router::{NotificationEvent, ReleaseContext};
use forage_core::integrations::{
    validate_integration_name, validate_webhook_url, CreateIntegrationInput, IntegrationConfig,
    IntegrationType,
};
use forage_core::platform::validate_slug;
use forage_core::session::CachedOrg;
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error};
use crate::auth::Session;
use crate::notification_worker::NotificationDispatcher;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{org}/settings/integrations",
            get(list_integrations),
        )
        .route(
            "/orgs/{org}/settings/integrations/install/webhook",
            get(install_webhook_page),
        )
        .route(
            "/orgs/{org}/settings/integrations/webhook",
            post(create_webhook),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}",
            get(integration_detail),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/rules",
            post(update_rules),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/toggle",
            post(toggle_integration),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/delete",
            post(delete_integration),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/test",
            post(test_integration),
        )
        .route(
            "/orgs/{org}/settings/integrations/install/slack",
            get(install_slack_page),
        )
        .route(
            "/orgs/{org}/settings/integrations/slack",
            post(create_slack),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/reinstall",
            post(reinstall_slack),
        )
        .route(
            "/integrations/slack/callback",
            get(slack_oauth_callback),
        )
}

fn require_org_membership<'a>(
    state: &AppState,
    orgs: &'a [CachedOrg],
    org: &str,
) -> Result<&'a CachedOrg, Response> {
    if !validate_slug(org) {
        return Err(error_page(
            state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid organisation name.",
        ));
    }
    orgs.iter().find(|o| o.name == org).ok_or_else(|| {
        error_page(
            state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You are not a member of this organisation.",
        )
    })
}

fn require_admin(state: &AppState, org: &CachedOrg) -> Result<(), Response> {
    if org.role == "owner" || org.role == "admin" {
        Ok(())
    } else {
        Err(error_page(
            state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You must be an admin to manage integrations.",
        ))
    }
}

fn require_integration_store(state: &AppState) -> Result<(), Response> {
    if state.integration_store.is_some() {
        Ok(())
    } else {
        Err(error_page(
            state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Integration management requires a database. Set DATABASE_URL to enable.",
        ))
    }
}

fn validate_csrf(session: &Session, form_csrf: &str) -> Result<(), Response> {
    if session.csrf_token == form_csrf {
        Ok(())
    } else {
        Err((
            axum::http::StatusCode::FORBIDDEN,
            "CSRF token mismatch",
        )
            .into_response())
    }
}

// ─── Query params ───────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize, Default)]
struct DetailQuery {
    #[serde(default)]
    test: Option<String>,
}

// ─── List integrations ──────────────────────────────────────────────

async fn list_integrations(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let store = state.integration_store.as_ref().unwrap();
    let integrations = store
        .list_integrations(&org)
        .await
        .map_err(|e| internal_error(&state, "list integrations", &e))?;

    // Build summary for each integration (count of enabled rules)
    let mut integration_summaries = Vec::new();
    for integ in &integrations {
        let rules = store
            .list_rules(&integ.id)
            .await
            .unwrap_or_default();
        let enabled_count = rules.iter().filter(|r| r.enabled).count();
        let total_count = rules.len();
        integration_summaries.push(context! {
            id => &integ.id,
            name => &integ.name,
            integration_type => integ.integration_type.as_str(),
            type_display => integ.integration_type.display_name(),
            enabled => integ.enabled,
            enabled_rules => enabled_count,
            total_rules => total_count,
            created_at => &integ.created_at,
        });
    }

    let html = state
        .templates
        .render(
            "pages/integrations.html.jinja",
            context! {
                title => format!("Integrations - {} - Forest", org),
                description => "Manage notification integrations",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integrations => integration_summaries,
                error => query.error,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Install webhook page ───────────────────────────────────────────

async fn install_webhook_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let html = state
        .templates
        .render(
            "pages/install_webhook.html.jinja",
            context! {
                title => format!("Install Webhook - {} - Forest", org),
                description => "Set up a webhook integration",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                error => query.error,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Create webhook ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateWebhookForm {
    _csrf: String,
    name: String,
    url: String,
    #[serde(default)]
    secret: String,
}

async fn create_webhook(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CreateWebhookForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    if let Err(e) = validate_integration_name(&form.name) {
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/webhook?error={}",
            org,
            urlencoding::encode(&e.to_string())
        ))
        .into_response());
    }

    if let Err(e) = validate_webhook_url(&form.url) {
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/webhook?error={}",
            org,
            urlencoding::encode(&e.to_string())
        ))
        .into_response());
    }

    let config = IntegrationConfig::Webhook {
        url: form.url,
        secret: if form.secret.is_empty() {
            None
        } else {
            Some(form.secret)
        },
        headers: std::collections::HashMap::new(),
    };

    let store = state.integration_store.as_ref().unwrap();
    let created = store
        .create_integration(&CreateIntegrationInput {
            organisation: org.clone(),
            integration_type: IntegrationType::Webhook,
            name: form.name,
            config,
            created_by: session.user.user_id.clone(),
        })
        .await
        .map_err(|e| internal_error(&state, "create webhook", &e))?;

    // Render the "installed" page directly (not a redirect) so we can show the API token once.
    // The raw token only exists in the create response and is never stored in plaintext.
    let html = state
        .templates
        .render(
            "pages/integration_installed.html.jinja",
            context! {
                title => format!("{} installed - Forest", created.name),
                description => "Integration installed successfully",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integration => context! {
                    id => &created.id,
                    name => &created.name,
                    type_display => created.integration_type.display_name(),
                },
                api_token => created.api_token,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Integration detail ─────────────────────────────────────────────

async fn integration_detail(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Query(query): Query<DetailQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let store = state.integration_store.as_ref().unwrap();
    let integration = store
        .get_integration(&org, &id)
        .await
        .map_err(|e| {
            error_page(
                &state,
                axum::http::StatusCode::NOT_FOUND,
                "Not found",
                &format!("Integration not found: {e}"),
            )
        })?;

    let rules = store.list_rules(&id).await.unwrap_or_default();
    let deliveries = store.list_deliveries(&id, 20).await.unwrap_or_default();

    let deliveries_ctx: Vec<_> = deliveries
        .iter()
        .map(|d| {
            context! {
                id => &d.id,
                notification_id => &d.notification_id,
                status => d.status.as_str(),
                error_message => &d.error_message,
                attempted_at => &d.attempted_at,
            }
        })
        .collect();

    let rules_ctx: Vec<_> = rules
        .iter()
        .map(|r| {
            context! {
                notification_type => &r.notification_type,
                label => notification_type_label(&r.notification_type),
                enabled => r.enabled,
            }
        })
        .collect();

    // Redact sensitive config fields for display
    let config_display = match &integration.config {
        IntegrationConfig::Slack {
            team_name,
            channel_name,
            webhook_url,
            ..
        } => {
            let detail = if team_name.is_empty() {
                format!("Webhook: {}", webhook_url)
            } else {
                format!("{} · {}", team_name, channel_name)
            };
            context! {
                type_name => "Slack",
                detail => detail,
            }
        }
        IntegrationConfig::Webhook { url, secret, .. } => context! {
            type_name => "Webhook",
            detail => url,
            has_secret => secret.is_some(),
        },
    };

    let html = state
        .templates
        .render(
            "pages/integration_detail.html.jinja",
            context! {
                title => format!("{} - Integrations - Forest", integration.name),
                description => "Integration settings",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integration => context! {
                    id => &integration.id,
                    name => &integration.name,
                    integration_type => integration.integration_type.as_str(),
                    type_display => integration.integration_type.display_name(),
                    enabled => integration.enabled,
                    created_at => &integration.created_at,
                },
                config => config_display,
                has_slack_oauth => state.slack_config.is_some(),
                rules => rules_ctx,
                deliveries => deliveries_ctx,
                test_sent => query.test.is_some(),
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Update notification rules ──────────────────────────────────────

#[derive(Deserialize)]
struct UpdateRuleForm {
    _csrf: String,
    notification_type: String,
    enabled: String,
}

async fn update_rules(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<UpdateRuleForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    let enabled = form.enabled == "true";
    let store = state.integration_store.as_ref().unwrap();

    // Verify integration belongs to org
    store
        .get_integration(&org, &id)
        .await
        .map_err(|e| internal_error(&state, "get integration", &e))?;

    store
        .set_rule_enabled(&id, &form.notification_type, enabled)
        .await
        .map_err(|e| internal_error(&state, "update rule", &e))?;

    Ok(Redirect::to(&format!(
        "/orgs/{}/settings/integrations/{}",
        org, id
    ))
    .into_response())
}

// ─── Toggle integration ─────────────────────────────────────────────

#[derive(Deserialize)]
struct ToggleForm {
    _csrf: String,
    enabled: String,
}

async fn toggle_integration(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<ToggleForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    let enabled = form.enabled == "true";
    let store = state.integration_store.as_ref().unwrap();
    store
        .set_integration_enabled(&org, &id, enabled)
        .await
        .map_err(|e| internal_error(&state, "toggle integration", &e))?;

    Ok(Redirect::to(&format!(
        "/orgs/{}/settings/integrations/{}",
        org, id
    ))
    .into_response())
}

// ─── Delete integration ─────────────────────────────────────────────

#[derive(Deserialize)]
struct CsrfForm {
    _csrf: String,
}

async fn delete_integration(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    let store = state.integration_store.as_ref().unwrap();
    store
        .delete_integration(&org, &id)
        .await
        .map_err(|e| internal_error(&state, "delete integration", &e))?;

    Ok(Redirect::to(&format!("/orgs/{}/settings/integrations", org)).into_response())
}

// ─── Test integration ───────────────────────────────────────────────

async fn test_integration(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    let store = state.integration_store.as_ref().unwrap();
    let integration = store
        .get_integration(&org, &id)
        .await
        .map_err(|e| internal_error(&state, "get integration", &e))?;

    // Build a test notification event
    let test_event = NotificationEvent {
        id: format!("test-{}", uuid::Uuid::new_v4()),
        notification_type: "release_succeeded".into(),
        title: "Test notification from Forest".into(),
        body: "This is a test notification to verify your integration is working.".into(),
        organisation: org.clone(),
        project: "test-project".into(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        release: Some(ReleaseContext {
            slug: "test-release".into(),
            artifact_id: "art_test".into(),
            release_intent_id: String::new(),
            destination: "staging".into(),
            environment: "staging".into(),
            source_username: session.user.username.clone(),
            source_user_id: session.user.user_id.clone(),
            commit_sha: "abc1234".into(),
            commit_branch: "main".into(),
            context_title: "Test notification from Forest".into(),
            context_web: String::new(),
            destination_count: 1,
            error_message: None,
        }),
    };

    let tasks = forage_core::integrations::router::route_notification(&test_event, &[integration]);
    let dispatcher = NotificationDispatcher::new(Arc::clone(store), String::new());
    for task in &tasks {
        dispatcher.dispatch(task).await;
    }

    Ok(Redirect::to(&format!(
        "/orgs/{}/settings/integrations/{}?test=sent",
        org, id
    ))
    .into_response())
}

// ─── Install Slack page ─────────────────────────────────────────────

async fn install_slack_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let slack_oauth_url = state.slack_config.as_ref().map(|sc| {
        format!(
            "https://slack.com/oauth/v2/authorize?client_id={}&scope=assistant:write,channels:join,chat:write,chat:write.public,im:history,im:read,im:write,incoming-webhook,links:read,links:write,reactions:write,users:read,users:read.email&redirect_uri={}/integrations/slack/callback&state={}",
            urlencoding::encode(&sc.client_id),
            urlencoding::encode(&sc.redirect_host),
            urlencoding::encode(&org),
        )
    });

    let html = state
        .templates
        .render(
            "pages/install_slack.html.jinja",
            context! {
                title => format!("Install Slack - {} - Forest", org),
                description => "Set up a Slack integration",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                error => query.error,
                slack_oauth_url => slack_oauth_url,
                has_slack_oauth => state.slack_config.is_some(),
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Create Slack (manual webhook URL fallback) ──────────────────────

#[derive(Deserialize)]
struct CreateSlackForm {
    _csrf: String,
    name: String,
    webhook_url: String,
    #[serde(default)]
    channel_name: String,
}

async fn create_slack(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CreateSlackForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    if let Err(e) = validate_integration_name(&form.name) {
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/slack?error={}",
            org,
            urlencoding::encode(&e.to_string())
        ))
        .into_response());
    }

    if let Err(e) = validate_slack_webhook_url(&form.webhook_url) {
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/slack?error={}",
            org,
            urlencoding::encode(&e)
        ))
        .into_response());
    }

    let channel = if form.channel_name.is_empty() {
        "#general".to_string()
    } else {
        form.channel_name
    };

    let config = IntegrationConfig::Slack {
        team_id: String::new(),
        team_name: String::new(),
        channel_id: String::new(),
        channel_name: channel,
        access_token: String::new(),
        webhook_url: form.webhook_url,
    };

    let store = state.integration_store.as_ref().unwrap();
    let created = store
        .create_integration(&CreateIntegrationInput {
            organisation: org.clone(),
            integration_type: IntegrationType::Slack,
            name: form.name,
            config,
            created_by: session.user.user_id.clone(),
        })
        .await
        .map_err(|e| internal_error(&state, "create slack", &e))?;

    let html = state
        .templates
        .render(
            "pages/integration_installed.html.jinja",
            context! {
                title => format!("{} installed - Forest", created.name),
                description => "Integration installed successfully",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integration => context! {
                    id => &created.id,
                    name => &created.name,
                    type_display => created.integration_type.display_name(),
                },
                api_token => created.api_token,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Reinstall Slack ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReinstallForm {
    _csrf: String,
}

async fn reinstall_slack(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<ReinstallForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;

    // Verify the integration exists and is a Slack integration
    let store = state.integration_store.as_ref().unwrap();
    let integration = store.get_integration(&org, &id).await.map_err(|e| {
        error_page(
            &state,
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
            &format!("Integration not found: {e}"),
        )
    })?;

    if integration.integration_type != IntegrationType::Slack {
        return Err(error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Only Slack integrations can be reinstalled via OAuth.",
        ));
    }

    let slack_config = state.slack_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not configured",
            "Slack OAuth is not configured. Set SLACK_CLIENT_ID and SLACK_CLIENT_SECRET.",
        )
    })?;

    // Encode org + integration ID in state so callback can update instead of create
    let oauth_state = format!("{}:reinstall:{}", org, id);
    let redirect_uri = format!(
        "{}/integrations/slack/callback",
        slack_config.redirect_host
    );
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope=assistant:write,channels:join,chat:write,chat:write.public,im:history,im:read,im:write,incoming-webhook,links:read,links:write,reactions:write,users:read,users:read.email&redirect_uri={}&state={}",
        urlencoding::encode(&slack_config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&oauth_state),
    );

    Ok(Redirect::to(&url).into_response())
}

// ─── Slack OAuth callback ────────────────────────────────────────────

#[derive(Deserialize)]
struct SlackCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn slack_oauth_callback(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<SlackCallbackQuery>,
) -> Result<Response, Response> {
    let raw_state = query.state.ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Missing state parameter from Slack callback.",
        )
    })?;

    // Parse state: either "{org}" (new install) or "{org}:reinstall:{id}" (reinstall)
    let (org, reinstall_id) = if raw_state.contains(":reinstall:") {
        let parts: Vec<&str> = raw_state.splitn(2, ":reinstall:").collect();
        (parts[0].to_string(), Some(parts[1].to_string()))
    } else {
        (raw_state, None)
    };

    // If Slack returned an error (user denied)
    if let Some(err) = query.error {
        let redirect_to = if let Some(ref rid) = reinstall_id {
            format!("/orgs/{}/settings/integrations/{}?error={}", org, rid, urlencoding::encode(&format!("Slack authorization denied: {err}")))
        } else {
            format!("/orgs/{}/settings/integrations/install/slack?error={}", org, urlencoding::encode(&format!("Slack authorization denied: {err}")))
        };
        return Ok(Redirect::to(&redirect_to).into_response());
    }

    let code = query.code.ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Missing authorization code from Slack.",
        )
    })?;

    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let slack_config = state.slack_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not configured",
            "Slack OAuth is not configured. Set SLACK_CLIENT_ID and SLACK_CLIENT_SECRET.",
        )
    })?;

    // Exchange code for token
    let http = reqwest::Client::new();
    let token_resp = http
        .post("https://slack.com/api/oauth.v2.access")
        .form(&[
            ("client_id", slack_config.client_id.as_str()),
            ("client_secret", slack_config.client_secret.as_str()),
            ("code", &code),
            (
                "redirect_uri",
                &format!("{}/integrations/slack/callback", slack_config.redirect_host),
            ),
        ])
        .send()
        .await
        .map_err(|e| {
            internal_error(&state, "slack oauth", &format!("Failed to contact Slack: {e}"))
        })?;

    let resp_body: serde_json::Value = token_resp.json().await.map_err(|e| {
        internal_error(
            &state,
            "slack oauth",
            &format!("Failed to parse Slack response: {e}"),
        )
    })?;

    if resp_body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err_msg = resp_body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/slack?error={}",
            org,
            urlencoding::encode(&format!("Slack error: {err_msg}"))
        ))
        .into_response());
    }

    // Extract fields from Slack response
    let team_id = resp_body["team"]["id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let team_name = resp_body["team"]["name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let access_token = resp_body["access_token"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let (channel_id, channel_name, webhook_url) =
        if let Some(wh) = resp_body.get("incoming_webhook") {
            (
                wh["channel_id"].as_str().unwrap_or("").to_string(),
                wh["channel"].as_str().unwrap_or("").to_string(),
                wh["url"].as_str().unwrap_or("").to_string(),
            )
        } else {
            (String::new(), String::new(), String::new())
        };

    let integration_name = if channel_name.is_empty() {
        format!("Slack - {team_name}")
    } else {
        format!("Slack - {channel_name}")
    };

    let config = IntegrationConfig::Slack {
        team_id,
        team_name,
        channel_id,
        channel_name,
        access_token,
        webhook_url,
    };

    let store = state.integration_store.as_ref().unwrap();

    if let Some(ref existing_id) = reinstall_id {
        // Reinstall: update existing integration's config
        store
            .update_integration_config(&org, existing_id, &integration_name, &config)
            .await
            .map_err(|e| internal_error(&state, "reinstall slack", &e))?;

        Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/{}",
            org, existing_id
        ))
        .into_response())
    } else {
        // New install: create integration
        let created = store
            .create_integration(&CreateIntegrationInput {
                organisation: org.clone(),
                integration_type: IntegrationType::Slack,
                name: integration_name,
                config,
                created_by: session.user.user_id.clone(),
            })
            .await
            .map_err(|e| internal_error(&state, "create slack", &e))?;

        let html = state
            .templates
            .render(
                "pages/integration_installed.html.jinja",
                context! {
                    title => format!("{} installed - Forest", created.name),
                    description => "Integration installed successfully",
                    user => context! {
                        username => &session.user.username,
                        user_id => &session.user.user_id,
                    },
                    current_org => &org,
                    orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                    csrf_token => &session.csrf_token,
                    active_tab => "settings",
                    integration => context! {
                        id => &created.id,
                        name => &created.name,
                        type_display => created.integration_type.display_name(),
                    },
                    api_token => created.api_token,
                },
            )
            .map_err(|e| internal_error(&state, "template error", &e))?;

        Ok(Html(html).into_response())
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

fn notification_type_label(nt: &str) -> &str {
    match nt {
        "release_annotated" => "Release annotated",
        "release_started" => "Release started",
        "release_succeeded" => "Release succeeded",
        "release_failed" => "Release failed",
        other => other,
    }
}

fn validate_slack_webhook_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://hooks.slack.com/")
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1")
    {
        Ok(())
    } else {
        Err("Slack webhook URL must start with https://hooks.slack.com/".to_string())
    }
}
