#![allow(clippy::result_large_err)]

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use forage_core::integrations::baseline::{MappingStatus, SlackBaseline};
use forage_core::integrations::direct::{
    SlackTeam, load_baseline, resolve_directory_match, sending_integration,
};
use forage_core::integrations::signing::{verify_github_signature, verify_slack_signature};
use forage_core::integrations::{
    CreateIntegrationInput, DeliveryStatus, Integration, IntegrationConfig, IntegrationError,
    IntegrationType,
};
use forage_core::session::CachedOrg;
use minijinja::context;
use serde::Deserialize;

use super::integrations::{
    require_admin, require_integration_store, require_org_membership, validate_csrf,
};
use super::{error_page, internal_error};
use crate::auth::Session;
use crate::state::AppState;

/// Coverage previews stop here: each member may cost a Slack lookup.
const MAX_COVERAGE_MEMBERS: usize = 100;
const GITHUB_INSTALL_STATE: &str = "github_install";
const GITHUB_VERIFY_STATE: &str = "github_install_verify";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{org}/settings/integrations/{id}/baseline",
            post(update_baseline),
        )
        .route(
            "/orgs/{org}/settings/integrations/{id}/coverage",
            get(coverage_page).post(refresh_coverage),
        )
        .route(
            "/orgs/{org}/settings/integrations/install/github",
            get(install_github_page),
        )
        .route(
            "/orgs/{org}/settings/integrations/github",
            post(start_github_install),
        )
        .route("/integrations/github/setup", get(github_setup))
        .route("/integrations/github/callback", get(github_callback))
        .route("/integrations/github/webhook", post(github_webhook))
        .route("/integrations/slack/events", post(slack_events))
}

pub(crate) fn new_nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn orgs_ctx(session: &Session) -> Vec<minijinja::Value> {
    session
        .user
        .orgs
        .iter()
        .map(|o| context! { name => &o.name, role => &o.role })
        .collect()
}

fn admin_org<'a>(
    state: &AppState,
    session: &'a Session,
    org: &str,
) -> Result<&'a CachedOrg, Response> {
    let cached_org = require_org_membership(state, &session.user.orgs, org)?;
    require_admin(state, cached_org)?;
    require_integration_store(state)?;
    Ok(cached_org)
}

async fn get_owned_integration(
    state: &AppState,
    org: &str,
    id: &str,
) -> Result<Integration, Response> {
    let store = state
        .integration_store
        .as_ref()
        .expect("checked by admin_org");
    store.get_integration(org, id).await.map_err(|_| {
        error_page(
            state,
            StatusCode::NOT_FOUND,
            "Not found",
            "Integration not found.",
        )
    })
}

fn slack_team(integration: &Integration, baseline: SlackBaseline) -> Option<SlackTeam> {
    match &integration.config {
        IntegrationConfig::Slack {
            team_id,
            access_token,
            ..
        } if !team_id.is_empty() && !access_token.is_empty() => Some(SlackTeam {
            integration: integration.clone(),
            team_id: team_id.clone(),
            access_token: access_token.clone(),
            baseline,
        }),
        _ => None,
    }
}

/// Refuses settings and coverage on an integration that does not send DMs for
/// its workspace, pointing at the one that does.
async fn require_sender(
    state: &AppState,
    org: &str,
    integration: &Integration,
) -> Result<(), Response> {
    let IntegrationConfig::Slack { team_id, .. } = &integration.config else {
        return Ok(());
    };
    let store = state.integration_store.as_ref().unwrap();
    match sending_integration(store.as_ref(), org, team_id).await {
        Some(sender) if sender.id != integration.id => Err(error_page(
            state,
            StatusCode::CONFLICT,
            "Configured elsewhere",
            &format!(
                "Individual notifications for this Slack workspace are sent and configured through \"{}\".",
                sender.name
            ),
        )),
        _ => Ok(()),
    }
}

#[derive(Deserialize)]
struct BaselineForm {
    _csrf: String,
    #[serde(default)]
    directory_mapping: Option<String>,
    #[serde(default)]
    dm_release_started: Option<String>,
    #[serde(default)]
    dm_release_succeeded: Option<String>,
    #[serde(default)]
    dm_release_failed: Option<String>,
}

async fn update_baseline(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<BaselineForm>,
) -> Result<Response, Response> {
    admin_org(&state, &session, &org)?;
    validate_csrf(&session, &form._csrf)?;
    let integration = get_owned_integration(&state, &org, &id).await?;
    if slack_team(&integration, SlackBaseline::legacy(&id, &[])).is_none() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Not available",
            "Individual notifications need a Slack workspace installed through Slack (not a webhook URL).",
        ));
    }
    require_sender(&state, &org, &integration).await?;

    let store = state.integration_store.as_ref().unwrap();
    let on = |v: &Option<String>| v.as_deref() == Some("on");
    let baseline = SlackBaseline {
        integration_id: id.clone(),
        directory_mapping: on(&form.directory_mapping),
        dm_defaults: [
            ("release_started", on(&form.dm_release_started)),
            ("release_succeeded", on(&form.dm_release_succeeded)),
            ("release_failed", on(&form.dm_release_failed)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
        updated_by: session.user.username.clone(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    store
        .upsert_slack_baseline(&baseline)
        .await
        .map_err(|e| internal_error(&state, "save slack baseline", &e))?;
    if !baseline.directory_mapping {
        let _ = store.delete_directory_matches(&id).await;
    }
    tracing::info!(
        org = %org,
        integration_id = %id,
        by = %session.user.user_id,
        directory_mapping = baseline.directory_mapping,
        "slack baseline settings changed"
    );

    Ok(Redirect::to(&format!("/orgs/{org}/settings/integrations/{id}?saved=1")).into_response())
}

struct CoverageRow {
    username: String,
    role: String,
    state: &'static str,
    detail: String,
}

async fn slack_coverage(
    state: &AppState,
    session: &Session,
    org: &str,
    cached_org: &CachedOrg,
    team: &SlackTeam,
    force: bool,
) -> Result<(Vec<CoverageRow>, bool), Response> {
    let store = state.integration_store.as_ref().unwrap();
    let members = state
        .platform_client
        .list_members(&session.access_token, &cached_org.organisation_id)
        .await
        .map_err(|e| internal_error(state, "list members", &e))?;
    let truncated = members.len() >= MAX_COVERAGE_MEMBERS;
    let now = chrono::Utc::now();

    let mut rows = Vec::new();
    for m in members.into_iter().take(MAX_COVERAGE_MEMBERS) {
        let personal = store
            .get_slack_user_link(&m.user_id, &team.team_id)
            .await
            .ok()
            .flatten();
        let (row_state, detail) = if personal.is_some() {
            (
                "reachable",
                "Personal Slack link (their own choice)".to_string(),
            )
        } else if !team.baseline.directory_mapping {
            (
                "off",
                "Not reached: org directory mapping is off".to_string(),
            )
        } else {
            let profile = match &state.recipient_directory {
                Some(dir) => dir.member(org, &m.user_id).await,
                None => Err("no Forest service account".into()),
            };
            match profile {
                Err(_) => (
                    "unknown",
                    "Could not check with Forest; try again".to_string(),
                ),
                Ok(None) => ("gap", "No longer a member".to_string()),
                Ok(Some(profile)) => match &state.slack_directory {
                    None => ("unknown", "Slack directory not configured".to_string()),
                    Some(slack) => {
                        match resolve_directory_match(
                            store.as_ref(),
                            slack.as_ref(),
                            team,
                            &profile,
                            now,
                            force,
                        )
                        .await
                        {
                            Ok(MappingStatus::Matched { .. }) => {
                                ("reachable", "Matched by verified Forest email".to_string())
                            }
                            Ok(other) => (
                                "gap",
                                format!(
                                    "{}. They can link Slack under Account settings.",
                                    capitalise(&other.describe())
                                ),
                            ),
                            Err(_) => ("unknown", "Slack lookup failed; try again".to_string()),
                        }
                    }
                },
            }
        };
        rows.push(CoverageRow {
            username: m.username,
            role: m.role,
            state: row_state,
            detail,
        });
    }
    Ok((rows, truncated))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

async fn render_coverage(
    state: &AppState,
    session: &Session,
    org: &str,
    id: &str,
    force: bool,
) -> Result<Response, Response> {
    let cached_org = admin_org(state, session, org)?;
    let integration = get_owned_integration(state, org, id).await?;
    require_sender(state, org, &integration).await?;
    let store = state.integration_store.as_ref().unwrap();
    let baseline = load_baseline(store.as_ref(), id).await;
    let Some(team) = slack_team(&integration, baseline) else {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Not available",
            "Coverage needs a Slack workspace installed through Slack.",
        ));
    };
    let (rows, truncated) = slack_coverage(state, session, org, cached_org, &team, force).await?;
    let reachable = rows.iter().filter(|r| r.state == "reachable").count();

    let html = state
        .templates
        .render(
            "pages/integration_coverage.html.jinja",
            context! {
                title => format!("Coverage - {} - Forest", integration.name),
                description => "Who receives individual Slack notifications",
                user => context! { username => &session.user.username, user_id => &session.user.user_id },
                current_org => org,
                orgs => orgs_ctx(session),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integration => context! { id => &integration.id, name => &integration.name, enabled => integration.enabled },
                directory_mapping => team.baseline.directory_mapping,
                total => rows.len(),
                reachable => reachable,
                truncated => truncated,
                max_members => MAX_COVERAGE_MEMBERS,
                rows => rows.iter().map(|r| context! {
                    username => &r.username,
                    role => &r.role,
                    state => r.state,
                    detail => &r.detail,
                }).collect::<Vec<_>>(),
            },
        )
        .map_err(|e| internal_error(state, "template error", &e))?;
    Ok(Html(html).into_response())
}

async fn coverage_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
) -> Result<Response, Response> {
    render_coverage(&state, &session, &org, &id, false).await
}

#[derive(Deserialize)]
struct CsrfOnly {
    _csrf: String,
}

async fn refresh_coverage(
    State(state): State<AppState>,
    session: Session,
    Path((org, id)): Path<(String, String)>,
    Form(form): Form<CsrfOnly>,
) -> Result<Response, Response> {
    validate_csrf(&session, &form._csrf)?;
    render_coverage(&state, &session, &org, &id, true).await
}

#[derive(Deserialize, Default)]
struct ErrorQuery {
    #[serde(default)]
    error: Option<String>,
}

async fn install_github_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<ErrorQuery>,
) -> Result<Response, Response> {
    admin_org(&state, &session, &org)?;
    let html = state
        .templates
        .render(
            "pages/install_github.html.jinja",
            context! {
                title => format!("Connect GitHub - {} - Forest", org),
                description => "Connect a GitHub organisation",
                user => context! { username => &session.user.username, user_id => &session.user.user_id },
                current_org => &org,
                orgs => orgs_ctx(&session),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                has_github_app => state.github_app.is_some(),
                error => query.error,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;
    Ok(Html(html).into_response())
}

async fn start_github_install(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CsrfOnly>,
) -> Result<Response, Response> {
    admin_org(&state, &session, &org)?;
    validate_csrf(&session, &form._csrf)?;
    let (Some(app), Some(states)) = (state.github_app.as_ref(), state.oauth_state_store.as_ref())
    else {
        return Err(error_page(
            &state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not configured",
            "GitHub organisation connections are not configured on this server.",
        ));
    };
    let nonce = new_nonce();
    states
        .create(
            GITHUB_INSTALL_STATE,
            &nonce,
            Some(&format!("{org}\n{}", session.user.user_id)),
            chrono::Utc::now() + chrono::Duration::minutes(15),
        )
        .await
        .map_err(|e| internal_error(&state, "github install state", &e))?;
    Ok(Redirect::to(&app.install_url(&nonce)).into_response())
}

fn install_refused(state: &AppState, message: &str) -> Response {
    error_page(
        state,
        StatusCode::FORBIDDEN,
        "GitHub connection refused",
        message,
    )
}

async fn consume_flow(
    state: &AppState,
    provider: &str,
    nonce: Option<&str>,
) -> Result<Vec<String>, Response> {
    let refused = || {
        install_refused(
            state,
            "This GitHub connection link is invalid, expired or was started by someone else. Start again from Forest.",
        )
    };
    let (Some(nonce), Some(states)) = (nonce, state.oauth_state_store.as_ref()) else {
        return Err(refused());
    };
    let flow = states
        .consume(provider, nonce)
        .await
        .map_err(|e| internal_error(state, "github install state", &e))?;
    flow.and_then(|f| f.return_to)
        .map(|r| r.split('\n').map(String::from).collect())
        .ok_or_else(refused)
}

#[derive(Deserialize)]
struct SetupQuery {
    installation_id: Option<String>,
    setup_action: Option<String>,
    state: Option<String>,
}

/// GitHub's setup redirect. The `installation_id` here is attacker-controllable,
/// so it is only remembered; ownership is proven in [`github_callback`].
async fn github_setup(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<SetupQuery>,
) -> Result<Response, Response> {
    let flow = consume_flow(&state, GITHUB_INSTALL_STATE, query.state.as_deref()).await?;
    let [org, started_by] = flow.as_slice() else {
        return Err(install_refused(&state, "Malformed installation state."));
    };
    if *started_by != session.user.user_id {
        return Err(install_refused(
            &state,
            "This installation was started by someone else.",
        ));
    }
    admin_org(&state, &session, org)?;
    if query.setup_action.as_deref() == Some("request") {
        return Ok(Redirect::to(&format!(
            "/orgs/{org}/settings/integrations/install/github?error={}",
            urlencoding::encode("Installation requested. A GitHub organisation owner must approve it, then connect again from here.")
        ))
        .into_response());
    }
    let installation_id: i64 = query
        .installation_id
        .as_deref()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| install_refused(&state, "GitHub did not return an installation."))?;

    let app = state.github_app.as_ref().ok_or_else(|| {
        install_refused(
            &state,
            "GitHub organisation connections are not configured.",
        )
    })?;
    let nonce = new_nonce();
    state
        .oauth_state_store
        .as_ref()
        .unwrap()
        .create(
            GITHUB_VERIFY_STATE,
            &nonce,
            Some(&format!(
                "{org}\n{}\n{installation_id}",
                session.user.user_id
            )),
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .await
        .map_err(|e| internal_error(&state, "github verify state", &e))?;
    let redirect_uri = format!("{}/integrations/github/callback", state.forage_host);
    Ok(Redirect::to(&app.authorize_url(&redirect_uri, &nonce)).into_response())
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Proves the installing GitHub user can access the installation, that it is
/// an organisation account with Members: read, and that they own that GitHub
/// organisation, then binds it to exactly one Forest organisation. The user
/// token is used for these checks only and is dropped here.
async fn github_callback(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, Response> {
    let flow = consume_flow(&state, GITHUB_VERIFY_STATE, query.state.as_deref()).await?;
    let [org, started_by, installation_id] = flow.as_slice() else {
        return Err(install_refused(&state, "Malformed installation state."));
    };
    if *started_by != session.user.user_id {
        return Err(install_refused(
            &state,
            "This installation was started by someone else.",
        ));
    }
    admin_org(&state, &session, org)?;
    let back = |msg: &str| {
        Redirect::to(&format!(
            "/orgs/{org}/settings/integrations/install/github?error={}",
            urlencoding::encode(msg)
        ))
        .into_response()
    };
    if query.error.is_some() {
        return Ok(back(
            "GitHub authorisation was cancelled; nothing was connected.",
        ));
    }
    let installation_id: i64 = installation_id
        .parse()
        .map_err(|_| install_refused(&state, "Malformed installation state."))?;
    let code = query
        .code
        .as_deref()
        .ok_or_else(|| install_refused(&state, "GitHub did not return an authorisation code."))?;
    let app = state.github_app.as_ref().ok_or_else(|| {
        install_refused(
            &state,
            "GitHub organisation connections are not configured.",
        )
    })?;

    let redirect_uri = format!("{}/integrations/github/callback", state.forage_host);
    let provider_failed = |e: String| {
        tracing::warn!(error = %e, "github installation verification failed");
        back("GitHub could not be reached to verify the installation. Try again.")
    };
    let user_token = match app.exchange_code(code, &redirect_uri).await {
        Ok(t) => t,
        Err(e) => return Ok(provider_failed(e)),
    };
    let installer = match app.user(&user_token).await {
        Ok(u) => u,
        Err(e) => return Ok(provider_failed(e)),
    };
    let accessible = match app.user_installation_ids(&user_token).await {
        Ok(ids) => ids,
        Err(e) => return Ok(provider_failed(e)),
    };
    drop(user_token);
    if !accessible.contains(&installation_id) {
        return Err(install_refused(
            &state,
            "That installation is not accessible to the GitHub account that authorised.",
        ));
    }
    let info = match app.installation(installation_id).await {
        Ok(Some(info)) => info,
        Ok(None) => {
            return Err(install_refused(
                &state,
                "The installation no longer exists on GitHub.",
            ));
        }
        Err(e) => return Ok(provider_failed(e)),
    };
    if info.account_type != "Organization" {
        return Err(install_refused(
            &state,
            "Install the app on a GitHub organisation, not a personal account.",
        ));
    }
    if info.suspended {
        return Err(install_refused(
            &state,
            "The installation is suspended on GitHub.",
        ));
    }
    if !matches!(
        info.members_permission.as_deref(),
        Some("read") | Some("write")
    ) {
        return Err(install_refused(
            &state,
            "The installation does not grant Members: read.",
        ));
    }
    let token = match app.installation_token(installation_id).await {
        Ok(t) => t,
        Err(e) => return Ok(provider_failed(e)),
    };
    let membership = match app
        .org_membership(&token, &info.account_login, &installer.login)
        .await
    {
        Ok(m) => m,
        Err(e) => return Ok(provider_failed(e)),
    };
    if membership.as_ref().map(|(r, s)| (r.as_str(), s.as_str())) != Some(("admin", "active")) {
        return Err(install_refused(
            &state,
            "Only an owner of the GitHub organisation can connect it.",
        ));
    }

    let store = state.integration_store.as_ref().unwrap();
    let config = IntegrationConfig::GitHub {
        installation_id,
        account_id: info.account_id,
        account_login: info.account_login.clone(),
        installed_by_login: installer.login.clone(),
        suspended: false,
    };
    let name = format!("GitHub - {}", info.account_login);
    let integration_id = match store
        .get_github_installation_binding(installation_id)
        .await
        .map_err(|e| internal_error(&state, "github binding", &e))?
    {
        Some((bound_org, _)) if bound_org != *org => {
            return Err(install_refused(
                &state,
                "This GitHub installation is already connected to another Forest organisation.",
            ));
        }
        Some((_, existing)) => {
            store
                .update_integration_config(org, &existing, &name, &config)
                .await
                .map_err(|e| internal_error(&state, "github update", &e))?;
            store
                .set_integration_enabled(org, &existing, true)
                .await
                .map_err(|e| internal_error(&state, "github update", &e))?;
            existing
        }
        None => {
            let created = store
                .create_integration(&CreateIntegrationInput {
                    organisation: org.clone(),
                    integration_type: IntegrationType::GitHub,
                    name,
                    config,
                    created_by: session.user.user_id.clone(),
                })
                .await
                .map_err(|e| match e {
                    IntegrationError::Duplicate(_) => install_refused(
                        &state,
                        "A GitHub connection with this name already exists.",
                    ),
                    e => internal_error(&state, "github create", &e),
                })?;
            if let Err(e) = store
                .bind_github_installation(installation_id, org, &created.id)
                .await
            {
                let _ = store.delete_integration(org, &created.id).await;
                return Err(match e {
                    IntegrationError::Duplicate(_) => install_refused(
                        &state,
                        "This GitHub installation is already connected to another Forest organisation.",
                    ),
                    e => internal_error(&state, "github bind", &e),
                });
            }
            created.id
        }
    };
    tracing::info!(org = %org, installation_id, by = %session.user.user_id, "github organisation connected");
    Ok(Redirect::to(&format!(
        "/orgs/{org}/settings/integrations/{integration_id}"
    ))
    .into_response())
}

pub(super) async fn github_connection_page(
    state: &AppState,
    session: &Session,
    org: &str,
    integration: &Integration,
) -> Result<Response, Response> {
    let IntegrationConfig::GitHub {
        installation_id,
        account_login,
        installed_by_login,
        ..
    } = &integration.config
    else {
        return Err(internal_error(
            state,
            "github page",
            &"not a GitHub integration",
        ));
    };
    let cached_org = admin_org(state, session, org)?;
    let store = state.integration_store.as_ref().unwrap();

    let (health, member_ids, truncated) = match &state.github_app {
        None => ("unconfigured", None, false),
        Some(app) => match app.installation(*installation_id).await {
            Ok(None) => {
                let _ = store
                    .set_integration_enabled(org, &integration.id, false)
                    .await;
                ("uninstalled", None, false)
            }
            Ok(Some(info)) if info.suspended => ("suspended", None, false),
            Ok(Some(_)) => match app.installation_token(*installation_id).await {
                Ok(token) => match app.org_member_ids(&token, account_login).await {
                    Ok((ids, truncated)) => ("healthy", Some(ids), truncated),
                    Err(_) => ("unreachable", None, false),
                },
                Err(_) => ("unreachable", None, false),
            },
            Err(_) => ("unreachable", None, false),
        },
    };

    let members = state
        .platform_client
        .list_members(&session.access_token, &cached_org.organisation_id)
        .await
        .map_err(|e| internal_error(state, "list members", &e))?;
    let mut rows = Vec::new();
    for m in members.iter().take(MAX_COVERAGE_MEMBERS) {
        let profile = match &state.recipient_directory {
            Some(dir) => dir.member(org, &m.user_id).await.ok().flatten(),
            None => None,
        };
        let (row_state, detail) = match (&profile, &member_ids) {
            (None, _) => ("unknown", "Could not check with Forest".to_string()),
            (Some(p), _) if p.github_user_ids.is_empty() => (
                "gap",
                "No GitHub account linked to Forest; releases they author on GitHub cannot be attributed to them. They can link GitHub under Account settings.".to_string(),
            ),
            (Some(_), None) => ("unknown", "GitHub membership could not be checked".to_string()),
            (Some(p), Some(ids)) if p.github_user_ids.iter().any(|id| ids.contains(id)) => (
                "reachable",
                format!("Linked GitHub account is a member of {account_login}"),
            ),
            (Some(_), Some(_)) => (
                "gap",
                format!("Linked GitHub account is not a member of {account_login}"),
            ),
        };
        rows.push(context! { username => &m.username, role => &m.role, state => row_state, detail => detail });
    }

    let html = state
        .templates
        .render(
            "pages/github_connection.html.jinja",
            context! {
                title => format!("{} - Integrations - Forest", integration.name),
                description => "GitHub organisation connection",
                user => context! { username => &session.user.username, user_id => &session.user.user_id },
                current_org => org,
                orgs => orgs_ctx(session),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                integration => context! { id => &integration.id, name => &integration.name, enabled => integration.enabled },
                account_login => account_login,
                installed_by_login => installed_by_login,
                health => health,
                truncated => truncated,
                rows => rows,
            },
        )
        .map_err(|e| internal_error(state, "template error", &e))?;
    Ok(Html(html).into_response())
}

/// GitHub App webhook. Only installation lifecycle events change anything;
/// GitHub membership never changes Forest membership.
async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(app) = state.github_app.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !verify_github_signature(&app.webhook_secret, &body, signature) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if event != "installation" {
        return StatusCode::NO_CONTENT.into_response();
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(installation_id) = payload["installation"]["id"].as_i64() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(store) = state.integration_store.as_ref() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let Ok(Some((org, integration_id))) =
        store.get_github_installation_binding(installation_id).await
    else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let action = payload["action"].as_str().unwrap_or_default();
    let message = match action {
        "deleted" => {
            let _ = store.delete_integration(&org, &integration_id).await;
            tracing::warn!(org = %org, installation_id, "github app uninstalled; connection removed");
            return StatusCode::NO_CONTENT.into_response();
        }
        "suspend" => "GitHub installation suspended",
        "unsuspend" => "GitHub installation unsuspended",
        _ => return StatusCode::NO_CONTENT.into_response(),
    };
    let suspended = action == "suspend";
    if let Ok(mut integration) = store.get_integration(&org, &integration_id).await
        && let IntegrationConfig::GitHub { suspended: s, .. } = &mut integration.config
    {
        *s = suspended;
        let _ = store
            .update_integration_config(
                &org,
                &integration_id,
                &integration.name,
                &integration.config,
            )
            .await;
        let _ = store
            .set_integration_enabled(&org, &integration_id, !suspended)
            .await;
    }
    let delivery = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("github");
    let _ = store
        .record_delivery(
            &integration_id,
            delivery,
            DeliveryStatus::Delivered,
            Some(message),
        )
        .await;
    StatusCode::NO_CONTENT.into_response()
}

/// Slack Events API: URL verification plus app uninstall / token revocation,
/// which disable every integration on that workspace.
async fn slack_events(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(secret) = state.slack_signing_secret.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    if !verify_slack_signature(
        secret,
        &header("x-slack-request-timestamp"),
        &body,
        &header("x-slack-signature"),
        chrono::Utc::now().timestamp(),
    ) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if payload["type"] == "url_verification" {
        return axum::Json(serde_json::json!({ "challenge": payload["challenge"] }))
            .into_response();
    }
    let event_type = payload["event"]["type"].as_str().unwrap_or_default();
    let revoked = match event_type {
        "app_uninstalled" => true,
        "tokens_revoked" => payload["event"]["tokens"]["bot"]
            .as_array()
            .is_some_and(|bots| !bots.is_empty()),
        _ => false,
    };
    let team_id = payload["team_id"].as_str().unwrap_or_default();
    if !revoked || team_id.is_empty() {
        return StatusCode::OK.into_response();
    }
    let Some(store) = state.integration_store.as_ref() else {
        return StatusCode::OK.into_response();
    };
    let integrations = store
        .list_integrations_of_type(IntegrationType::Slack)
        .await
        .unwrap_or_default();
    for integration in integrations {
        let IntegrationConfig::Slack { team_id: t, .. } = &integration.config else {
            continue;
        };
        if t != team_id {
            continue;
        }
        let _ = store
            .set_integration_enabled(&integration.organisation, &integration.id, false)
            .await;
        let _ = store.delete_directory_matches(&integration.id).await;
        let _ = store
            .record_delivery(
                &integration.id,
                &format!("slack-{event_type}"),
                DeliveryStatus::Failed,
                Some("Slack app was uninstalled or its token revoked; reinstall to resume"),
            )
            .await;
        tracing::warn!(org = %integration.organisation, integration_id = %integration.id, event_type, "slack integration revoked by workspace");
    }
    StatusCode::OK.into_response()
}
