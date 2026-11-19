use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use forage_core::integrations::router::{NotificationEvent, ReleaseContext};
use forage_core::integrations::slack_release::{EMOJI_SLOTS, SlackEmojiSet};
use forage_core::integrations::{
    validate_integration_name, validate_webhook_url, CreateIntegrationInput, IntegrationConfig,
    IntegrationType,
};
use forage_core::platform::validate_slug;
use forage_core::session::CachedOrg;
use serde::Deserialize;

use super::{error_page, internal_error, report_error};
use crate::auth::Session;
use crate::notification_worker::NotificationDispatcher;
use crate::state::AppState;
use crate::views::org_settings::{
    BaselineView, ConfigView, EmojiField, InstallSlackPage, InstallWebhookPage,
    IntegrationDetailPage, IntegrationInstalledPage, IntegrationRow, IntegrationsPage, RuleView,
    SentBy, chrome as org_chrome,
};

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
            "/orgs/{org}/settings/integrations/{id}/emoji",
            post(update_emoji),
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
            "/orgs/{org}/settings/integrations/install/slack/start",
            post(start_slack_install),
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

pub(super) fn require_org_membership<'a>(
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

pub(super) fn require_admin(state: &AppState, org: &CachedOrg) -> Result<(), Response> {
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

pub(super) fn require_integration_store(state: &AppState) -> Result<(), Response> {
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

pub(super) fn validate_csrf(session: &Session, form_csrf: &str) -> Result<(), Response> {
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

#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize, Default)]
struct DetailQuery {
    #[serde(default)]
    test: Option<String>,
    #[serde(default)]
    saved: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

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

    let mut rows = Vec::new();
    for integ in integrations {
        let rules = store.list_rules(&integ.id).await.unwrap_or_default();
        rows.push(IntegrationRow {
            enabled_rules: rules.iter().filter(|r| r.enabled).count(),
            total_rules: rules.len(),
            id: integ.id,
            name: integ.name,
            kind: integ.integration_type,
            enabled: integ.enabled,
        });
    }

    let page = IntegrationsPage {
        chrome: org_chrome(
            &state,
            &session,
            &org,
            format!("Integrations - {} - Forest", org),
            "Manage notification integrations".into(),
        ),
        org: org.clone(),
        error: query.error,
        integrations: rows,
    };
    Ok(Html(crate::views::pages::integrations::render(&page)).into_response())
}

async fn install_webhook_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let page = InstallWebhookPage {
        chrome: org_chrome(
            &state,
            &session,
            &org,
            format!("Install Webhook - {} - Forest", org),
            "Set up a webhook integration".into(),
        ),
        org: org.clone(),
        csrf_token: session.csrf_token.clone(),
        error: query.error,
    };
    Ok(Html(crate::views::pages::install_webhook::render(&page)).into_response())
}

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

    Ok(installed_page(&state, &session, &org, created))
}

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
    let integration = store.get_integration(&org, &id).await.map_err(|e| {
        error_page(
            &state,
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
            &report_error("get integration", &e),
        )
    })?;

    let rules = store.list_rules(&id).await.unwrap_or_default();
    let deliveries = store.list_deliveries(&id, 20).await.unwrap_or_default();

    let rules: Vec<RuleView> = rules
        .iter()
        .map(|r| RuleView {
            notification_type: r.notification_type.clone(),
            label: notification_type_label(&r.notification_type).to_string(),
            enabled: r.enabled,
        })
        .collect();

    let config = match &integration.config {
        IntegrationConfig::Slack {
            team_name,
            channel_name,
            webhook_url,
            ..
        } => ConfigView::Slack {
            detail: if team_name.is_empty() {
                format!("Webhook: {}", webhook_url)
            } else {
                format!("{} · {}", team_name, channel_name)
            },
        },
        IntegrationConfig::Webhook { url, secret, .. } => ConfigView::Webhook {
            url: url.clone(),
            has_secret: secret.is_some(),
        },
        IntegrationConfig::GitHub { .. } => {
            return super::org_connections::github_connection_page(&state, &session, &org, &integration)
                .await;
        }
    };
    let sender = match &integration.config {
        IntegrationConfig::Slack { team_id, .. } if !team_id.is_empty() => {
            forage_core::integrations::direct::sending_integration(store.as_ref(), &org, team_id)
                .await
                .filter(|s| s.id != integration.id)
        }
        _ => None,
    };
    let is_slack = integration.integration_type == IntegrationType::Slack;
    let baseline = if is_slack {
        let b = forage_core::integrations::direct::load_baseline(store.as_ref(), &id).await;
        Some(BaselineView {
            sent_by: sender.map(|s| SentBy { id: s.id, name: s.name }),
            directory_mapping: b.directory_mapping,
            defaults: forage_core::integrations::baseline::DM_NOTIFICATION_TYPES
                .iter()
                .map(|nt| RuleView {
                    notification_type: nt.to_string(),
                    label: notification_type_label(nt).to_string(),
                    enabled: b.org_default(nt),
                })
                .collect(),
            updated_by: b.updated_by,
            updated_at: b.updated_at,
        })
    } else {
        None
    };

    let emoji = if is_slack {
        let set = store
            .get_slack_emoji_set(&org)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        EMOJI_SLOTS
            .iter()
            .map(|(field, fallback, label)| EmojiField {
                field,
                fallback,
                label,
                value: set.configured(field).to_string(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let page = IntegrationDetailPage {
        chrome: org_chrome(
            &state,
            &session,
            &org,
            format!("{} - Integrations - Forest", integration.name),
            "Integration settings".into(),
        ),
        org: org.clone(),
        csrf_token: session.csrf_token.clone(),
        can_reinstall: is_slack && state.slack_config.is_some(),
        id: integration.id,
        name: integration.name,
        kind: integration.integration_type,
        enabled: integration.enabled,
        config,
        rules,
        deliveries,
        test_sent: query.test.is_some(),
        baseline,
        emoji,
        emoji_saved: query.saved.as_deref() == Some("emoji"),
        saved: query.saved.as_deref().is_some_and(|s| s != "emoji"),
        error: query.error,
    };
    Ok(Html(crate::views::pages::integration_detail::render(&page)).into_response())
}

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

async fn update_emoji(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<std::collections::BTreeMap<String, String>>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, form.get("_csrf").map(String::as_str).unwrap_or(""))?;

    let store = state.integration_store.as_ref().unwrap();
    let integration = store
        .get_integration(&org, &id)
        .await
        .map_err(|e| internal_error(&state, "get integration", &e))?;
    if integration.integration_type != IntegrationType::Slack {
        return Err(error_page(
            &state,
            axum::http::StatusCode::BAD_REQUEST,
            "Not available",
            "Status emoji apply to Slack integrations only.",
        ));
    }

    let Ok(set) = SlackEmojiSet::from_fields(&form) else {
        return Ok(Redirect::to(&format!(
            "/orgs/{org}/settings/integrations/{id}?error=Emoji+names+may+only+use+lowercase+letters%2C+digits+and+-+_+%2B+%27#emoji"
        ))
        .into_response());
    };
    store
        .set_slack_emoji_set(&org, &set, &session.user.username)
        .await
        .map_err(|e| internal_error(&state, "save slack emoji", &e))?;
    tracing::info!(org = %org, by = %session.user.user_id, "slack status emoji changed");

    Ok(Redirect::to(&format!("/orgs/{org}/settings/integrations/{id}?saved=emoji#emoji")).into_response())
}

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
            source_attribution: "actor".into(),
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

async fn install_slack_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;

    let page = InstallSlackPage {
        chrome: org_chrome(
            &state,
            &session,
            &org,
            format!("Install Slack - {} - Forest", org),
            "Set up a Slack integration".into(),
        ),
        org: org.clone(),
        csrf_token: session.csrf_token.clone(),
        error: query.error,
        has_slack_oauth: state.slack_config.is_some(),
    };
    Ok(Html(crate::views::pages::install_slack::render(&page)).into_response())
}

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

    Ok(installed_page(&state, &session, &org, created))
}

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

    let store = state.integration_store.as_ref().unwrap();
    let integration = store.get_integration(&org, &id).await.map_err(|e| {
        error_page(
            &state,
            axum::http::StatusCode::NOT_FOUND,
            "Not found",
            &report_error("get Slack integration for reinstall", &e),
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

    let url = slack_authorize_redirect(&state, slack_config, &session, &org, Some(&id)).await?;
    Ok(Redirect::to(&url).into_response())
}

const SLACK_INSTALL_STATE: &str = "slack_install";

/// Existing bot scopes, unchanged by org baseline delivery, which needs only
/// `chat:write`, `users:read` and `users:read.email` of these.
const SLACK_BOT_SCOPES: &str = "assistant:write,channels:join,chat:write,chat:write.public,im:history,im:read,im:write,incoming-webhook,links:read,links:write,reactions:write,users:read,users:read.email";

/// Binds a Slack install to this admin, organisation and (for a reinstall)
/// integration with a single-use server-side nonce.
async fn slack_authorize_redirect(
    state: &AppState,
    slack_config: &crate::state::SlackConfig,
    session: &Session,
    org: &str,
    reinstall_id: Option<&str>,
) -> Result<String, Response> {
    let store = state.oauth_state_store.as_ref().ok_or_else(|| {
        error_page(
            state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "Slack installation needs the OAuth state store.",
        )
    })?;
    let nonce = super::org_connections::new_nonce();
    let return_to = format!(
        "{org}\n{}\n{}",
        session.user.user_id,
        reinstall_id.unwrap_or_default()
    );
    store
        .create(
            SLACK_INSTALL_STATE,
            &nonce,
            Some(&return_to),
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .await
        .map_err(|e| internal_error(state, "slack install state", &e))?;
    let redirect_uri = format!("{}/integrations/slack/callback", slack_config.redirect_host);
    Ok(format!(
        "{}?client_id={}&scope={}&redirect_uri={}&state={}",
        state.slack_authorize_url,
        urlencoding::encode(&slack_config.client_id),
        SLACK_BOT_SCOPES,
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&nonce),
    ))
}

async fn start_slack_install(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_integration_store(&state)?;
    validate_csrf(&session, &form._csrf)?;
    let slack_config = state.slack_config.as_ref().ok_or_else(|| {
        error_page(
            &state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not configured",
            "Slack OAuth is not configured. Set SLACK_CLIENT_ID and SLACK_CLIENT_SECRET.",
        )
    })?;
    let url = slack_authorize_redirect(&state, slack_config, &session, &org, None).await?;
    Ok(Redirect::to(&url).into_response())
}

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

    let invalid_state = || {
        error_page(
            &state,
            axum::http::StatusCode::FORBIDDEN,
            "Installation not verified",
            "This Slack installation link is invalid, expired or was started by someone else. Start again from Forest.",
        )
    };
    let flow = match state.oauth_state_store.as_ref() {
        Some(store) => store
            .consume(SLACK_INSTALL_STATE, &raw_state)
            .await
            .map_err(|e| internal_error(&state, "slack install state", &e))?,
        None => None,
    };
    let Some(return_to) = flow.and_then(|f| f.return_to) else {
        return Err(invalid_state());
    };
    let mut parts = return_to.splitn(3, '\n');
    let (org, started_by, reinstall_id) = (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().filter(|s| !s.is_empty()).map(String::from),
    );
    if started_by != session.user.user_id {
        return Err(invalid_state());
    }

    if query.error.is_some() {
        let redirect_to = if let Some(ref rid) = reinstall_id {
            format!(
                "/orgs/{}/settings/integrations/{}?error={}",
                org,
                rid,
                urlencoding::encode("Slack authorization was denied. Please try again.")
            )
        } else {
            format!(
                "/orgs/{}/settings/integrations/install/slack?error={}",
                org,
                urlencoding::encode("Slack authorization was denied. Please try again.")
            )
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

    let http = reqwest::Client::new();
    let token_resp = http
        .post(state.slack_api.url("oauth.v2.access"))
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
            urlencoding::encode(&report_error("Slack OAuth response", &err_msg))
        ))
        .into_response());
    }

    if resp_body["is_enterprise_install"].as_bool() == Some(true) {
        return Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/install/slack?error={}",
            org,
            urlencoding::encode("Enterprise Grid org-wide installs are not supported; install into one workspace.")
        ))
        .into_response());
    }

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

    let new_team_id = team_id.clone();
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
        let existing = store.get_integration(&org, existing_id).await.map_err(|_| {
            error_page(
                &state,
                axum::http::StatusCode::NOT_FOUND,
                "Not found",
                "Integration not found.",
            )
        })?;
        if let IntegrationConfig::Slack { team_id: old_team, .. } = &existing.config
            && !old_team.is_empty()
            && *old_team != new_team_id
        {
            return Ok(Redirect::to(&format!(
                "/orgs/{}/settings/integrations/{}?error={}",
                org,
                existing_id,
                urlencoding::encode("Reinstall was authorised for a different Slack workspace; nothing changed.")
            ))
            .into_response());
        }
        store
            .update_integration_config(&org, existing_id, &integration_name, &config)
            .await
            .map_err(|e| internal_error(&state, "reinstall slack", &e))?;
        store
            .set_integration_enabled(&org, existing_id, true)
            .await
            .map_err(|e| internal_error(&state, "reinstall slack", &e))?;
        let _ = store.delete_directory_matches(existing_id).await;

        Ok(Redirect::to(&format!(
            "/orgs/{}/settings/integrations/{}",
            org, existing_id
        ))
        .into_response())
    } else {
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

        Ok(installed_page(&state, &session, &org, created))
    }
}

fn installed_page(
    state: &AppState,
    session: &Session,
    org: &str,
    created: forage_core::integrations::Integration,
) -> Response {
    let page = IntegrationInstalledPage {
        chrome: org_chrome(
            state,
            session,
            org,
            format!("{} installed - Forest", created.name),
            "Integration installed successfully".into(),
        ),
        org: org.to_string(),
        type_display: created.integration_type.display_name(),
        id: created.id,
        name: created.name,
        api_token: created.api_token,
    };
    Html(crate::views::pages::integration_installed::render(&page)).into_response()
}

pub(super) fn notification_type_label(nt: &str) -> &str {
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
