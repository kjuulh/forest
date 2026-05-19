use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::Form;
use chrono::Datelike;
use forage_core::platform::{
    validate_slug, CreatePolicyInput, CreateReleasePipelineInput, CreateTriggerInput,
    PipelineStage, PolicyConfig, UpdatePolicyInput, UpdateReleasePipelineInput,
    UpdateTriggerInput,
};
use forage_core::session::CachedOrg;
use minijinja::context;
use serde::{Deserialize, Serialize};

use super::{error_page, internal_error, warn_default};
use crate::auth::{self, Session};
use crate::manifest_view::ManifestView;
use crate::pretty_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/notifications", get(notifications_page))
        .route("/orgs", post(create_org_submit))
        .route("/orgs/{org}/projects", get(projects_list))
        .route("/orgs/{org}/projects/{project}", get(project_detail))
        .route(
            "/orgs/{org}/projects/{project}/releases",
            get(project_releases),
        )
        // Legacy /deployments URL — kept as a redirect to /releases for
        // external links. The deployment timeline (and CD action chips)
        // now live under "Releases" since a release IS a deployment.
        .route(
            "/orgs/{org}/projects/{project}/deployments",
            get(deployments_to_releases_redirect),
        )
        .route(
            "/orgs/{org}/projects/{project}/releases/{slug}",
            get(artifact_detail),
        )
        .route("/orgs/{org}/releases", get(releases_page))
        .route("/orgs/{org}/destinations", get(destinations_page))
        .route(
            "/orgs/{org}/destinations/environments",
            post(create_environment_submit),
        )
        .route(
            "/orgs/{org}/destinations/create",
            post(create_destination_submit),
        )
        .route(
            "/orgs/{org}/destinations/detail",
            get(destination_detail),
        )
        .route(
            "/orgs/{org}/destinations/detail/update",
            post(update_destination_submit),
        )
        .route("/orgs/{org}/settings", get(org_settings))
        .route("/orgs/{org}/settings/usage", get(usage))
        .route("/orgs/{org}/settings/compute", get(compute_page))
        // Legacy redirects
        .route("/orgs/{org}/usage", get(redirect_usage))
        .route("/orgs/{org}/compute", get(redirect_compute))
        .route(
            "/orgs/{org}/settings/members",
            get(members_page).post(add_member_submit),
        )
        .route(
            "/orgs/{org}/settings/members/{user_id}/role",
            post(update_member_role_submit),
        )
        .route(
            "/orgs/{org}/settings/members/{user_id}/remove",
            post(remove_member_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/deploy",
            post(deploy_release),
        )
        .route(
            "/orgs/{org}/projects/{project}/triggers",
            get(triggers_page).post(create_trigger_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/triggers/{name}",
            get(edit_trigger_page).post(edit_trigger_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/triggers/{name}/toggle",
            post(toggle_trigger),
        )
        .route(
            "/orgs/{org}/projects/{project}/triggers/{name}/delete",
            post(delete_trigger),
        )
        .route(
            "/orgs/{org}/projects/{project}/policies",
            get(policies_page).post(create_policy_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/policies/{name}",
            get(edit_policy_page).post(edit_policy_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/policies/{name}/toggle",
            post(toggle_policy),
        )
        .route(
            "/orgs/{org}/projects/{project}/policies/{name}/delete",
            post(delete_policy),
        )
        .route(
            "/orgs/{org}/projects/{project}/releases/{slug}/approve",
            post(approve_release_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/releases/{slug}/reject",
            post(reject_release_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/pipelines",
            get(pipelines_page).post(create_pipeline_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/pipelines/{name}/toggle",
            post(toggle_pipeline),
        )
        .route(
            "/orgs/{org}/projects/{project}/pipelines/{name}/update",
            post(update_pipeline_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/pipelines/{name}/delete",
            post(delete_pipeline),
        )
        .route("/users/{username}", get(user_profile))
        .route(
            "/api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/approve",
            post(approve_plan_stage_submit),
        )
        .route(
            "/api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/reject",
            post(reject_plan_stage_submit),
        )
        .route(
            "/api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/output",
            get(get_plan_output_api),
        )
        .route(
            "/api/orgs/{org}/projects/{project}/timeline",
            get(timeline_api),
        )
        .route("/api/orgs/{org}/timeline", get(org_timeline_api))
        .route(
            "/orgs/{org}/settings/compute/rollouts/{rollout_id}",
            get(rollout_detail_page),
        )
        .route("/api/compute/regions", get(regions_api))
}

fn orgs_context(orgs: &[CachedOrg]) -> Vec<minijinja::Value> {
    orgs.iter()
        .map(|o| context! { name => o.name, role => o.role })
        .collect()
}


#[allow(clippy::result_large_err)]
fn require_org_membership<'a>(
    state: &AppState,
    orgs: &'a [CachedOrg],
    org: &str,
) -> Result<&'a CachedOrg, Response> {
    if !validate_slug(org) {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid organisation name.",
        ));
    }
    orgs.iter().find(|o| o.name == org).ok_or_else(|| {
        error_page(
            state,
            StatusCode::FORBIDDEN,
            "Access denied",
            "You don't have access to this organisation.",
        )
    })
}

/// Require the user to be an admin or owner of the organisation.
#[allow(clippy::result_large_err)]
fn require_admin(state: &AppState, org: &CachedOrg) -> Result<(), Response> {
    if org.role == "owner" || org.role == "admin" {
        Ok(())
    } else {
        Err(error_page(
            state,
            StatusCode::FORBIDDEN,
            "Access denied",
            "You must be an admin to perform this action.",
        ))
    }
}

// ─── Dashboard ──────────────────────────────────────────────────────

async fn dashboard(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;

    if orgs.is_empty() {
        // No orgs: show onboarding with create org form
        let html = state
            .templates
            .render(
                "pages/onboarding.html.jinja",
                context! {
                    title => "Get Started - Forage",
                    description => "Create your first organisation",
                    user => context! { username => session.user.username },
                    csrf_token => &session.csrf_token,
                    active_tab => "dashboard",
                },
            )
            .map_err(|e| {
                internal_error(&state, "template error", &e)
            })?;
        return Ok(Html(html).into_response());
    }

    // Fetch recent activity: for each org, get projects, then artifacts
    let mut recent_activity = Vec::new();
    let mut first_org_projects: Vec<String> = Vec::new();
    for org in orgs {
        let projects = warn_default(
            "dashboard: list_projects",
            state.platform_client.list_projects(&session.access_token, &org.name).await,
        );

        if first_org_projects.is_empty() && org.name == orgs.first().map(|o| o.name.as_str()).unwrap_or_default() {
            first_org_projects = projects.clone();
        }

        for project in projects.iter().take(5) {
            let artifacts = warn_default(
                "dashboard: list_artifacts",
                state.platform_client.list_artifacts(&session.access_token, &org.name, project).await,
            );

            for artifact in artifacts {
                let mut seen_envs = std::collections::HashSet::new();
                let dest_envs: Vec<String> = artifact
                    .destinations
                    .iter()
                    .filter(|d| seen_envs.insert(d.environment.clone()))
                    .map(|d| d.environment.clone())
                    .collect();
                recent_activity.push(context! {
                    org_name => org.name,
                    project_name => project,
                    slug => artifact.slug,
                    title => artifact.context.title,
                    created_at => artifact.created_at,
                    dest_envs => dest_envs,
                });
                if recent_activity.len() >= 10 {
                    break;
                }
            }
            if recent_activity.len() >= 10 {
                break;
            }
        }
    }

    let html = state
        .templates
        .render(
            "pages/dashboard.html.jinja",
            context! {
                title => "Dashboard - Forage",
                description => "Your Forage dashboard",
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => orgs.first().map(|o| &o.name),
                orgs => orgs_context(orgs),
                projects => first_org_projects,
                recent_activity => recent_activity,
                active_tab => "dashboard",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

// ─── Notifications ───────────────────────────────────────────────────

struct NotifRelease {
    org: String,
    project: String,
    slug: String,
    title: String,
    description: Option<String>,
    version: Option<String>,
    branch: Option<String>,
    commit_sha: Option<String>,
    commit_message: Option<String>,
    source_user: Option<String>,
    created_at: String,
    summary_status: String,
    env_groups: Vec<minijinja::Value>,
    pipeline_stages: Vec<minijinja::Value>,
    has_pipeline: bool,
    destinations: Vec<minijinja::Value>,
}

async fn fetch_notifications(
    state: &AppState,
    session: &Session,
) -> Vec<NotifRelease> {
    let orgs = &session.user.orgs;
    let username = &session.user.username;
    let mut releases: Vec<NotifRelease> = Vec::new();

    for org in orgs {
        let (projects, dest_states, release_intents) = tokio::join!(
            state
                .platform_client
                .list_projects(&session.access_token, &org.name),
            state
                .platform_client
                .get_destination_states(&session.access_token, &org.name, None),
            state
                .platform_client
                .get_release_intent_states(&session.access_token, &org.name, None, true),
        );
        let projects = match projects {
            Ok(p) => p,
            Err(_) => continue,
        };
        let dest_states = dest_states.unwrap_or_default();
        let release_intents = release_intents.unwrap_or_default();

        // Index destination states by artifact_id.
        let mut states_by_artifact: std::collections::HashMap<
            &str,
            Vec<&forage_core::platform::DestinationState>,
        > = std::collections::HashMap::new();
        for ds in &dest_states.destinations {
            if let Some(aid) = ds.artifact_id.as_deref() {
                states_by_artifact.entry(aid).or_default().push(ds);
            }
        }

        // Index pipeline stages by artifact_id.
        let mut intent_stages_by_artifact: std::collections::HashMap<
            &str,
            &[forage_core::platform::PipelineRunStageState],
        > = std::collections::HashMap::new();
        for ri in &release_intents {
            if !ri.stages.is_empty() {
                intent_stages_by_artifact.insert(ri.artifact_id.as_str(), &ri.stages);
            }
        }

        // Fetch pipeline configs per project to know which projects have pipelines.
        let mut pipelines_by_project: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        for p in &projects {
            let has = warn_default(
                "list_release_pipelines",
                state
                    .platform_client
                    .list_release_pipelines(&session.access_token, &org.name, p)
                    .await,
            )
            .iter()
            .any(|pl| pl.enabled);
            if has {
                pipelines_by_project.insert(p.clone(), true);
            }
        }

        for project in &projects {
            let artifacts = match state
                .platform_client
                .list_artifacts(&session.access_token, &org.name, project)
                .await
            {
                Ok(a) => a,
                Err(_) => continue,
            };
            for artifact in artifacts {
                // Filter to current user's releases.
                let is_mine = artifact
                    .source
                    .as_ref()
                    .and_then(|s| s.user.as_deref())
                    .map(|u| u == username)
                    .unwrap_or(false);
                if !is_mine {
                    continue;
                }

                let matching_states = states_by_artifact
                    .get(artifact.artifact_id.as_str())
                    .cloned()
                    .unwrap_or_default();

                // Compute summary status.
                let aid = artifact.artifact_id.as_str();
                let summary_status = compute_summary_status(&matching_states, || {
                    intent_stages_by_artifact.contains_key(aid)
                });

                // Build env groups for display.
                let env_groups = build_env_groups(&matching_states);

                // Build pipeline stages from intent data.
                let mut pipeline_stages: Vec<minijinja::Value> = Vec::new();
                if let Some(run_stages) = intent_stages_by_artifact.get(aid) {
                    let sorted = topo_sort_run_stages(run_stages);
                    for rs in sorted {
                        let base_status = deploy_stage_display_status(rs, &matching_states);
                        let display_status = if rs.stage_type == "plan" && rs.approval_status.as_deref() == Some("AWAITING_APPROVAL") {
                            "AWAITING_APPROVAL"
                        } else {
                            base_status
                        };
                        pipeline_stages.push(context! {
                            id => rs.stage_id,
                            stage_type => rs.stage_type,
                            environment => rs.environment,
                            duration_seconds => rs.duration_seconds,
                            status => display_status,
                            started_at => rs.started_at,
                            completed_at => rs.completed_at,
                            error_message => rs.error_message,
                            wait_until => rs.wait_until,
                            approval_status => rs.approval_status,
                        });
                    }
                }

                let project_has_pipeline = pipelines_by_project.contains_key(project);
                let has_pipeline = !pipeline_stages.is_empty() || project_has_pipeline;

                // Build destinations.
                let destinations: Vec<minijinja::Value> = matching_states
                    .iter()
                    .map(|ds| {
                        context! {
                            name => ds.destination_name,
                            environment => ds.environment,
                            status => ds.status,
                            error_message => ds.error_message,
                            queued_at => ds.queued_at,
                            started_at => ds.started_at,
                            completed_at => ds.completed_at,
                            queue_position => ds.queue_position,
                        }
                    })
                    .collect();

                releases.push(NotifRelease {
                    org: org.name.clone(),
                    project: project.clone(),
                    slug: artifact.slug,
                    title: artifact.context.title,
                    description: artifact.context.description,
                    version: artifact.git_ref.as_ref().and_then(|r| r.version.clone()),
                    branch: artifact.git_ref.as_ref().and_then(|r| r.branch.clone()),
                    commit_sha: artifact
                        .git_ref
                        .as_ref()
                        .map(|r| r.commit_sha[..r.commit_sha.len().min(7)].to_string()),
                    commit_message: artifact
                        .git_ref
                        .as_ref()
                        .and_then(|r| r.commit_message.clone()),
                    source_user: artifact.source.as_ref().and_then(|s| s.user.clone()),
                    created_at: artifact.created_at,
                    summary_status: summary_status.to_string(),
                    env_groups,
                    pipeline_stages,
                    has_pipeline,
                    destinations,
                });
            }
        }
    }

    // Sort: in-progress first (RUNNING, QUEUED), then by created_at descending.
    releases.sort_by(|a, b| {
        let active = |s: &str| matches!(s, "RUNNING" | "QUEUED");
        let a_active = active(&a.summary_status);
        let b_active = active(&b.summary_status);
        match (a_active, b_active) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.created_at.cmp(&a.created_at),
        }
    });

    releases.truncate(50);
    releases
}

fn notifications_to_values(releases: Vec<NotifRelease>) -> Vec<minijinja::Value> {
    releases
        .into_iter()
        .map(|r| {
            context! {
                org => r.org,
                project => r.project,
                slug => r.slug,
                title => r.title,
                description => r.description,
                version => r.version,
                branch => r.branch,
                commit_sha => r.commit_sha,
                commit_message => r.commit_message,
                source_user => r.source_user,
                created_at => r.created_at,
                summary_status => r.summary_status,
                env_groups => r.env_groups,
                pipeline_stages => r.pipeline_stages,
                has_pipeline => r.has_pipeline,
                destinations => r.destinations,
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct NotificationsQuery {
    #[serde(default)]
    _partial: Option<String>,
}

// ─── Org settings ──────────────────────────────────────────────────

async fn org_settings(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    let current_role = current_org.role.clone();

    let html = state
        .templates
        .render(
            "pages/org_settings.html.jinja",
            context! {
                title => format!("{org} - Settings - Forage"),
                description => format!("Settings for {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                current_role => &current_role,
                active_tab => "settings",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

async fn redirect_usage(Path(org): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/orgs/{org}/settings/usage"))
}

async fn redirect_compute(Path(org): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/orgs/{org}/settings/compute"))
}

async fn notifications_page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<NotificationsQuery>,
) -> Result<Response, Response> {
    let releases = fetch_notifications(&state, &session).await;
    let release_values = notifications_to_values(releases);

    // Partial render: return just the list HTML for AJAX polling.
    if query._partial.is_some() {
        let html = state
            .templates
            .render(
                "components/notifications_list.html.jinja",
                context! { releases => release_values },
            )
            .map_err(|e| internal_error(&state, "template error", &e))?;
        return Ok(Html(html).into_response());
    }

    let orgs = &session.user.orgs;
    let html = state
        .templates
        .render(
            "pages/notifications.html.jinja",
            context! {
                title => "Notifications - Forage",
                description => "Your release activity",
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => orgs.first().map(|o| &o.name),
                orgs => orgs_context(orgs),
                releases => release_values,
                active_tab => "notifications",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Create organisation ────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateOrgForm {
    name: String,
    _csrf: String,
}

async fn create_org_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CreateOrgForm>,
) -> Result<Response, Response> {
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    if !validate_slug(&form.name) {
        // Re-render onboarding/dashboard with error
        let html = state
            .templates
            .render(
                "pages/onboarding.html.jinja",
                context! {
                    title => "Get Started - Forage",
                    description => "Create your first organisation",
                    user => context! { username => session.user.username },
                    csrf_token => &session.csrf_token,
                    active_tab => "dashboard",
                    error => "Invalid organisation name. Use lowercase letters, numbers, and hyphens only.",
                },
            )
            .map_err(|e| {
                internal_error(&state, "template error", &e)
            })?;
        return Ok(Html(html).into_response());
    }

    match state
        .platform_client
        .create_organisation(&session.access_token, &form.name)
        .await
    {
        Ok(org_id) => {
            // Update session with new org
            if let Ok(Some(mut session_data)) = state.sessions.get(&session.session_id).await {
                if let Some(ref mut user) = session_data.user {
                    user.orgs.push(CachedOrg {
                        organisation_id: org_id,
                        name: form.name.clone(),
                        role: "owner".into(),
                    });
                }
                let _ = state
                    .sessions
                    .update(&session.session_id, session_data)
                    .await;
            }
            Ok(Redirect::to(&format!("/orgs/{}/projects", form.name)).into_response())
        }
        Err(e) => {
            tracing::error!("failed to create org: {e}");
            let html = state
                .templates
                .render(
                    "pages/onboarding.html.jinja",
                    context! {
                        title => "Get Started - Forage",
                        description => "Create your first organisation",
                        user => context! { username => session.user.username },
                        csrf_token => &session.csrf_token,
                        active_tab => "dashboard",
                        error => "Could not create organisation. Please try again.",
                    },
                )
                .map_err(|e| {
                    tracing::error!("template error: {e:#}");
                    error_page(&state, StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong", "Please try again.")
                })?;
            Ok(Html(html).into_response())
        }
    }
}

// ─── Projects list ──────────────────────────────────────────────────

async fn projects_list(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    let projects = state
        .platform_client
        .list_projects(&session.access_token, &org)
        .await
        .map_err(|e| internal_error(&state, "list_projects", &e))?;

    let html = state
        .templates
        .render(
            "pages/projects.html.jinja",
            context! {
                title => format!("{org} - Projects - Forage"),
                description => format!("Projects in {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                projects => projects,
                active_tab => "projects",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

// ─── Project detail ─────────────────────────────────────────────────

async fn project_detail(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    let current_role = current_org.role.clone();

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let component_versions_fut = async {
        match state.registry_client.as_ref() {
            Some(registry) => registry
                .list_component_versions(&session.access_token, &org, &project)
                .await,
            None => Ok(vec![]),
        }
    };
    // Per specs/features/008: the project Overview folds in the canonical
    // component's detail (shape, tool facet, README, manifest) when the
    // 1:1 name match holds. If the component doesn't exist yet (fresh
    // project) or the call fails, the Overview renders the Get-started
    // panel — no error to the user.
    let comp_detail_fut = async {
        match state.registry_client.as_ref() {
            Some(registry) => registry
                .get_component_detail(&session.access_token, &org, &project)
                .await
                .ok(),
            None => None,
        }
    };
    let (
        artifacts,
        projects,
        environments,
        dest_states,
        release_intents,
        project_pipelines,
        component_versions,
        comp_detail,
    ) = tokio::join!(
        state
            .platform_client
            .list_artifacts(&session.access_token, &org, &project),
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, Some(&project)),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, Some(&project), true),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
        component_versions_fut,
        comp_detail_fut,
    );
    let artifacts = artifacts.map_err(|e| internal_error(&state, "list_artifacts", &e))?;
    let projects = warn_default("list_projects", projects);
    let environments = warn_default("list_environments", environments);
    let dest_states = warn_default("get_destination_states", dest_states);
    let release_intents = warn_default("get_release_intent_states", release_intents);
    let project_pipelines = warn_default("list_release_pipelines", project_pipelines);
    let component_versions = warn_default("list_component_versions", component_versions);

    // Environment options for the deploy dropdown (sorted by sort_order).
    let mut sorted_envs = environments.clone();
    sorted_envs.sort_by_key(|e| e.sort_order);
    let env_options: Vec<minijinja::Value> = if !sorted_envs.is_empty() {
        sorted_envs
            .iter()
            .map(|e| context! { name => e.name })
            .collect()
    } else {
        // Fallback: derive from artifact destinations
        let mut env_seen = std::collections::HashSet::new();
        artifacts
            .iter()
            .flat_map(|a| a.destinations.iter())
            .filter(|d| env_seen.insert(d.environment.clone()))
            .map(|d| context! { name => d.environment })
            .collect()
    };

    let items: Vec<ArtifactWithProject> = artifacts
        .into_iter()
        .map(|a| ArtifactWithProject {
            artifact: a,
            project_name: project.clone(),
        })
        .collect();
    let mut pipelines_map = PipelinesByProject::new();
    if !project_pipelines.is_empty() {
        pipelines_map.insert(project.clone(), project_pipelines);
    }
    let data = build_timeline(items, &org, &environments, &dest_states, &release_intents, &pipelines_map);


    // Project Overview folds in the canonical component's catalog data.
    // When the component exists: shape badge, install copy, README markdown,
    // manifest pretty JSON + structured view. When it doesn't, all of these
    // are None/empty and the template shows the Get-started panel.
    let comp_summary = comp_detail.as_ref().map(|d| d.summary.clone());
    let comp_versions = comp_detail.as_ref().map(|d| d.versions.clone()).unwrap_or_default();
    let comp_readme_html = comp_detail
        .as_ref()
        .filter(|d| !d.readme.is_empty())
        .map(|d| super::render_markdown(&d.readme))
        .unwrap_or_default();
    let manifest_raw = comp_detail
        .as_ref()
        .map(|d| d.manifest_json.clone())
        .unwrap_or_default();
    let manifest_html = if manifest_raw.is_empty() {
        String::new()
    } else {
        pretty_json::tokenize(&manifest_raw)
    };
    let manifest_view = ManifestView::parse(&manifest_raw);

    let html = state
        .templates
        .render(
            "pages/project_detail.html.jinja",
            context! {
                title => format!("{project} - {org} - Forage"),
                description => format!("Project {project} in {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                project_name => &project,
                projects => projects,
                current_role => &current_role,
                active_tab => "project_overview",
                timeline => data.timeline,
                lanes => data.lanes,
                env_options => env_options,
                component_versions => component_versions,
                summary => comp_summary,
                comp_versions => &comp_versions,
                readme_html => comp_readme_html,
                manifest_html => manifest_html,
                manifest => manifest_view,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}


// ─── Project releases (deployment timeline + CD plumbing) ────────────
//
// One canonical "Releases" tab shows every release performed for a
// project: the timeline of which artefacts went to which environments,
// plus links to Pipelines / Triggers / Policies management. The legacy
// /deployments URL 303-redirects here.

async fn project_releases(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    let current_role = current_org.role.clone();

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (
        artifacts,
        projects,
        environments,
        dest_states,
        release_intents,
        project_pipelines,
    ) = tokio::join!(
        state
            .platform_client
            .list_artifacts(&session.access_token, &org, &project),
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, Some(&project)),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, Some(&project), true),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
    );
    let artifacts = artifacts.map_err(|e| internal_error(&state, "list_artifacts", &e))?;
    let projects = warn_default("list_projects", projects);
    let environments = warn_default("list_environments", environments);
    let dest_states = warn_default("get_destination_states", dest_states);
    let release_intents = warn_default("get_release_intent_states", release_intents);
    let project_pipelines = warn_default("list_release_pipelines", project_pipelines);

    let mut sorted_envs = environments.clone();
    sorted_envs.sort_by_key(|e| e.sort_order);
    let env_options: Vec<minijinja::Value> = if !sorted_envs.is_empty() {
        sorted_envs
            .iter()
            .map(|e| context! { name => e.name })
            .collect()
    } else {
        let mut env_seen = std::collections::HashSet::new();
        artifacts
            .iter()
            .flat_map(|a| a.destinations.iter())
            .filter(|d| env_seen.insert(d.environment.clone()))
            .map(|d| context! { name => d.environment })
            .collect()
    };

    let items: Vec<ArtifactWithProject> = artifacts
        .into_iter()
        .map(|a| ArtifactWithProject {
            artifact: a,
            project_name: project.clone(),
        })
        .collect();
    let mut pipelines_map = PipelinesByProject::new();
    if !project_pipelines.is_empty() {
        pipelines_map.insert(project.clone(), project_pipelines);
    }
    let data = build_timeline(items, &org, &environments, &dest_states, &release_intents, &pipelines_map);

    let html = state
        .templates
        .render(
            "pages/project_releases.html.jinja",
            context! {
                title => format!("Releases - {project} - {org} - Forage"),
                description => format!("Releases performed for {project} in {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                project_name => &project,
                projects => projects,
                current_role => &current_role,
                active_tab => "project_releases",
                timeline => data.timeline,
                lanes => data.lanes,
                env_options => env_options,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// Legacy `/orgs/{org}/projects/{project}/deployments` — 303s to the
/// consolidated Releases tab. Kept for external-link survival.
async fn deployments_to_releases_redirect(
    Path((org, project)): Path<(String, String)>,
) -> Response {
    Redirect::to(&format!("/orgs/{org}/projects/{project}/releases"))
        .into_response()
}

// ─── Artifact detail ─────────────────────────────────────────────────

async fn artifact_detail(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, slug)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (artifact_result, projects, dest_states, release_intents, pipelines, environments) = tokio::join!(
        state
            .platform_client
            .get_artifact_by_slug(&session.access_token, &slug),
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, Some(&project)),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, Some(&project), true),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
    );
    // Fetch artifact spec after we have the artifact_id (needs artifact_result first).

    let artifact = artifact_result.map_err(|e| match e {
        forage_core::platform::PlatformError::NotFound(_) => error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Not found",
            "This release could not be found.",
        ),
        other => {
            internal_error(&state, "failed to fetch artifact", &other)
        }
    })?;

    // Fetch artifact spec now that we have the artifact_id.
    let artifact_spec = state
        .platform_client
        .get_artifact_spec(&session.access_token, &artifact.artifact_id)
        .await
        .unwrap_or_default();

    let projects = warn_default("list_projects", projects);
    let dest_states = dest_states.unwrap_or_default();
    let release_intents = release_intents.unwrap_or_default();
    let project_has_pipeline = warn_default("list_release_pipelines", pipelines)
        .iter()
        .any(|pl| pl.enabled);

    // Filter destination states to this artifact.
    let matching_states: Vec<&forage_core::platform::DestinationState> = dest_states
        .destinations
        .iter()
        .filter(|ds| ds.artifact_id.as_deref() == Some(&artifact.artifact_id))
        .collect();

    // Compute summary status.
    let summary_status = compute_summary_status(&matching_states, || {
        release_intents
            .iter()
            .any(|ri| ri.artifact_id == artifact.artifact_id && !ri.stages.is_empty())
    });

    // Build pipeline stages from the most recent release intent for this artifact.
    let mut pipeline_stages: Vec<minijinja::Value> = Vec::new();
    let latest_intent = release_intents
        .iter()
        .filter(|ri| ri.artifact_id == artifact.artifact_id && !ri.stages.is_empty())
        .max_by_key(|ri| &ri.created_at);

    if let Some(ri) = latest_intent {
        let sorted = topo_sort_run_stages(&ri.stages);
        for rs in sorted {
            let base_status = deploy_stage_display_status(rs, &matching_states);
            let display_status = if rs.stage_type == "plan" && rs.approval_status.as_deref() == Some("AWAITING_APPROVAL") {
                "AWAITING_APPROVAL"
            } else {
                base_status
            };
            pipeline_stages.push(context! {
                id => rs.stage_id,
                stage_type => rs.stage_type,
                environment => rs.environment,
                duration_seconds => rs.duration_seconds,
                status => display_status,
                started_at => rs.started_at,
                completed_at => rs.completed_at,
                error_message => rs.error_message,
                wait_until => rs.wait_until,
                approval_status => rs.approval_status,
            });
        }
    }

    let has_pipeline = !pipeline_stages.is_empty() || project_has_pipeline;

    // Fetch policy evaluations for active release intents.
    struct PolicyEvalEntry {
        policy_name: String,
        policy_type: String,
        passed: bool,
        reason: String,
        target_environment: String,
        approval_state: Option<forage_core::platform::ApprovalState>,
    }

    let mut raw_evals: Vec<PolicyEvalEntry> = Vec::new();
    let release_intent_id_str = latest_intent
        .map(|ri| ri.release_intent_id.clone())
        .unwrap_or_default();
    let is_release_author = false;
    if let Some(ri) = latest_intent {
        {
            let mut seen = std::collections::BTreeSet::new();
            let environments: Vec<String> = ri
                .stages
                .iter()
                .filter_map(|s| s.environment.clone())
                .filter(|e| seen.insert(e.clone()))
                .collect();

            for env in &environments {
                if let Ok(evals) = state
                    .platform_client
                    .evaluate_policies(
                        &session.access_token,
                        &org,
                        &project,
                        env,
                        Some(&ri.release_intent_id),
                    )
                    .await
                {
                    for eval in evals {
                        raw_evals.push(PolicyEvalEntry {
                            policy_name: eval.policy_name,
                            policy_type: eval.policy_type,
                            passed: eval.passed,
                            reason: eval.reason,
                            target_environment: env.clone(),
                            approval_state: eval.approval_state,
                        });
                    }
                }
            }
        }
    }

    raw_evals.sort_by(|a, b| a.policy_type.cmp(&b.policy_type).then(a.policy_name.cmp(&b.policy_name)));

    let policy_evaluations: Vec<minijinja::Value> = raw_evals
        .iter()
        .map(|eval| {
            let approval_state_ctx = eval.approval_state.as_ref().map(|s| {
                let decisions: Vec<minijinja::Value> = s
                    .decisions
                    .iter()
                    .map(|d| {
                        context! {
                            username => d.username,
                            decision => d.decision,
                            comment => d.comment,
                            decided_at => d.decided_at,
                        }
                    })
                    .collect();
                context! {
                    required_approvals => s.required_approvals,
                    current_approvals => s.current_approvals,
                    decisions => decisions,
                }
            });
            context! {
                policy_name => eval.policy_name,
                policy_type => eval.policy_type,
                passed => eval.passed,
                reason => eval.reason,
                target_environment => eval.target_environment,
                approval_state => approval_state_ctx,
            }
        })
        .collect();

    let current_org_entry = orgs.iter().find(|o| o.name == org);
    let is_admin = current_org_entry
        .map(|o| o.role == "owner" || o.role == "admin")
        .unwrap_or(false);

    // Build env groups.
    let env_groups = build_env_groups(&matching_states);

    // Build destinations with status.
    let destinations: Vec<minijinja::Value> = matching_states
        .iter()
        .map(|ds| {
            context! {
                name => ds.destination_name,
                environment => ds.environment,
                status => ds.status,
                error_message => ds.error_message,
                queued_at => ds.queued_at,
                started_at => ds.started_at,
                completed_at => ds.completed_at,
                queue_position => ds.queue_position,
            }
        })
        .collect();

    let artifact_id_val = artifact.artifact_id.clone();

    let html = state
        .templates
        .render(
            "pages/artifact_detail.html.jinja",
            context! {
                title => format!("{} - {} - {} - Forage", artifact.context.title, project, org),
                description => artifact.context.description,
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                project_name => &project,
                projects => projects,
                active_tab => "project_releases",
                artifact => {
                    // Parse auto-generated description for fallback metadata.
                    let desc_meta = artifact.context.description.as_deref()
                        .filter(|d| d.starts_with("Branch:"))
                        .map(parse_description_metadata)
                        .unwrap_or_default();

                    let branch = artifact.git_ref.as_ref().and_then(|r| r.branch.clone())
                        .or_else(|| desc_meta.get("branch").cloned());
                    let source_type = artifact.source.as_ref().and_then(|s| s.source_type.clone())
                        .or_else(|| desc_meta.get("source").cloned());
                    let source_user = artifact.source.as_ref().and_then(|s| s.user.clone())
                        .or_else(|| desc_meta.get("author").cloned());

                    context! {
                        slug => artifact.slug,
                        title => artifact.context.title,
                        description => artifact.context.description,
                        web => artifact.context.web,
                        pr => artifact.context.pr,
                        created_at => artifact.created_at,
                        source_user => source_user,
                        source_email => artifact.source.as_ref().and_then(|s| s.email.clone()),
                        source_type => source_type,
                        run_url => artifact.source.as_ref().and_then(|s| s.run_url.clone()),
                        commit_sha => artifact.git_ref.as_ref().map(|r| r.commit_sha.clone()),
                        branch => branch,
                        commit_message => artifact.git_ref.as_ref().and_then(|r| r.commit_message.clone()),
                        version => artifact.git_ref.as_ref().and_then(|r| r.version.clone()),
                        repo_url => artifact.git_ref.as_ref().and_then(|r| r.repo_url.clone()),
                    }
                },
                summary_status => &summary_status,
                pipeline_stages => pipeline_stages,
                has_pipeline => has_pipeline,
                env_groups => env_groups,
                destinations => destinations,
                configured_destinations => artifact.destinations.iter().map(|d| {
                    context! { name => d.name, environment => d.environment }
                }).collect::<Vec<_>>(),
                has_release_intents => release_intents.iter().any(|ri| ri.artifact_id == artifact.artifact_id),
                artifact_spec => if artifact_spec.is_empty() { None::<String> } else { Some(artifact_spec) },
                policy_evaluations => policy_evaluations,
                release_intent_id => &release_intent_id_str,
                is_release_author => is_release_author,
                is_admin => is_admin,
                artifact_id => &artifact_id_val,
                has_active_pipeline => has_pipeline,
                environments => warn_default("list_environments", environments)
                    .iter()
                    .map(|e| context! { name => e.name })
                    .collect::<Vec<_>>(),
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

/// Compute summary status from destination states.
fn compute_summary_status<F: FnOnce() -> bool>(
    matching_states: &[&forage_core::platform::DestinationState],
    has_intent_stages: F,
) -> String {
    if matching_states.is_empty() {
        if has_intent_stages() {
            "QUEUED".to_string()
        } else {
            "PENDING".to_string()
        }
    } else {
        let statuses: Vec<&str> = matching_states
            .iter()
            .filter_map(|ds| ds.status.as_deref())
            .collect();
        if statuses.iter().any(|s| *s == "RUNNING" || *s == "ASSIGNED") {
            "RUNNING"
        } else if statuses.contains(&"QUEUED") {
            "QUEUED"
        } else if statuses.contains(&"FAILED") {
            "FAILED"
        } else if statuses.contains(&"TIMED_OUT") {
            "TIMED_OUT"
        } else if statuses.contains(&"CANCELLED") {
            "CANCELLED"
        } else if statuses.contains(&"SUCCEEDED") {
            "SUCCEEDED"
        } else {
            "PENDING"
        }
        .to_string()
    }
}

/// Destination-aware status override for deploy stages.
fn deploy_stage_display_status<'a>(
    rs: &'a forage_core::platform::PipelineRunStageState,
    matching_states: &[&forage_core::platform::DestinationState],
) -> &'a str {
    if rs.stage_type == "deploy" && (rs.status == "RUNNING" || rs.status == "ASSIGNED") {
        if let Some(ref env) = rs.environment {
            let env_dests: Vec<&str> = matching_states
                .iter()
                .filter(|ds| ds.environment == *env)
                .filter_map(|ds| ds.status.as_deref())
                .collect();
            if !env_dests.is_empty() && env_dests.iter().all(|s| *s == "QUEUED") {
                return "QUEUED";
            }
        }
    }
    &rs.status
}

/// Parse auto-generated description like "Branch: main. Source: github_actions. Author: tnielsen."
/// into a map of key-value pairs. Used as fallback when structured fields are empty.
fn parse_description_metadata(desc: &str) -> std::collections::HashMap<String, String> {
    let mut meta = std::collections::HashMap::new();
    for part in desc.split(". ") {
        let part = part.trim_end_matches('.');
        if let Some((key, val)) = part.split_once(": ") {
            meta.insert(key.to_lowercase(), val.to_string());
        }
    }
    meta
}

/// Build env groups for display (grouped by best status).
fn build_env_groups(
    matching_states: &[&forage_core::platform::DestinationState],
) -> Vec<minijinja::Value> {
    let mut env_best: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    let mut unique_envs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ds in matching_states {
        let status = ds.status.as_deref().unwrap_or("PENDING");
        let env = ds.environment.as_str();
        if seen.insert(env) {
            unique_envs.push(env);
        }
        let cur = env_best.get(env).copied().unwrap_or("PENDING");
        let pri = |s: &str| -> u8 {
            match s {
                "RUNNING" | "ASSIGNED" => 6,
                "QUEUED" => 5,
                "FAILED" => 4,
                "TIMED_OUT" => 3,
                "CANCELLED" => 2,
                "SUCCEEDED" => 1,
                _ => 0,
            }
        };
        if pri(status) > pri(cur) {
            env_best.insert(env, status);
        }
    }
    let status_order = [
        "RUNNING", "QUEUED", "FAILED", "TIMED_OUT", "CANCELLED", "SUCCEEDED",
    ];
    let mut env_groups = Vec::new();
    for &gs in &status_order {
        let envs_in: Vec<&str> = unique_envs
            .iter()
            .filter(|e| env_best.get(*e).copied() == Some(gs))
            .copied()
            .collect();
        if !envs_in.is_empty() {
            let ds = if gs == "ASSIGNED" { "RUNNING" } else { gs };
            env_groups.push(context! {
                status => ds,
                envs => envs_in,
            });
        }
    }
    env_groups
}

// ─── Usage ──────────────────────────────────────────────────────────

async fn usage(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let _ = require_org_membership(&state, orgs, &org)?;

    let projects = warn_default("list_projects", state
        .platform_client
        .list_projects(&session.access_token, &org)
        .await);

    let html = state
        .templates
        .render(
            "pages/usage.html.jinja",
            context! {
                title => format!("Usage - {org} - Forage"),
                description => format!("Usage and plan for {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                project_count => projects.len(),
                active_tab => "settings",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

// ─── Deploy release ────────────────────────────────────────────────

#[derive(Deserialize)]
struct DeployForm {
    _csrf: String,
    artifact_id: String,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    use_pipeline: Option<String>,
}

async fn deploy_release(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<DeployForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed.",
        ));
    }

    let use_pipeline = form.use_pipeline.as_deref() == Some("true");
    let environments: Vec<String> = form.environment.into_iter().collect();

    state
        .platform_client
        .release_artifact(
            &session.access_token,
            &form.artifact_id,
            &[],
            &environments,
            use_pipeline,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "deploy failed", &e)
        })?;

    Ok(Redirect::to(&format!(
        "/orgs/{org}/projects/{project}/releases"
    ))
    .into_response())
}

// ─── User profile ──────────────────────────────────────────────────

async fn user_profile(
    State(state): State<AppState>,
    session: Session,
    Path(username): Path<String>,
) -> Result<Response, Response> {
    let profile = state
        .forest_client
        .get_user_by_username(&session.access_token, &username)
        .await
        .map_err(|e| {
{
                tracing::error!("get_user_by_username({username}): {e:#}");
                error_page(
                    &state,
                    StatusCode::NOT_FOUND,
                    "User not found",
                    &format!("No user named '{username}' was found."),
                )
            }
        })?;

    let orgs = &session.user.orgs;

    // Fetch contributions: collect artifacts created by this user across all orgs/projects.
    let profile_data = build_user_profile_data(&state, &session, orgs, &username).await;

    let html = state
        .templates
        .render(
            "pages/user_profile.html.jinja",
            context! {
                title => format!("{} - Forage", profile.username),
                description => format!("Profile for {}", profile.username),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => orgs.first().map(|o| &o.name),
                orgs => orgs_context(orgs),
                profile => context! {
                    username => profile.username,
                    user_id => profile.user_id,
                    profile_picture_url => profile.profile_picture_url,
                    created_at => profile.created_at,
                },
                heatmap => profile_data.heatmap,
                recent_releases => profile_data.recent_releases,
                contributed_projects => profile_data.contributed_projects,
                active_tab => "",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

/// User profile data: heatmap, recent releases, and contributed projects.
struct UserProfileData {
    heatmap: minijinja::Value,
    recent_releases: Vec<minijinja::Value>,
    contributed_projects: Vec<minijinja::Value>,
}

/// Build user profile data: contribution heatmap, recent releases, and contributed projects.
async fn build_user_profile_data(
    state: &AppState,
    session: &Session,
    orgs: &[forage_core::session::CachedOrg],
    target_username: &str,
) -> UserProfileData {
    use std::collections::{HashMap, HashSet};

    let today = chrono::Utc::now().date_naive();
    let start = today - chrono::Duration::days(363);
    let start_weekday = start.weekday().num_days_from_sunday();
    let grid_start = start - chrono::Duration::days(start_weekday as i64);

    let mut day_counts: HashMap<chrono::NaiveDate, u32> = HashMap::new();
    let mut total_contributions = 0u32;

    // Collect recent releases (sorted by created_at desc, capped at 10).
    struct RecentRelease {
        org: String,
        project: String,
        slug: String,
        version: Option<String>,
        branch: Option<String>,
        commit_sha: Option<String>,
        created_at: String,
    }
    let mut all_releases: Vec<RecentRelease> = Vec::new();

    // Track unique org/project pairs for contributed projects.
    let mut project_set: HashSet<(String, String)> = HashSet::new();
    let mut project_release_counts: HashMap<(String, String), u32> = HashMap::new();

    // Fetch artifacts from all orgs/projects (best effort).
    for org in orgs {
        let projects = match state
            .platform_client
            .list_projects(&session.access_token, &org.name)
            .await
        {
            Ok(p) => p,
            Err(_) => continue,
        };
        for project in &projects {
            let artifacts = match state
                .platform_client
                .list_artifacts(&session.access_token, &org.name, project)
                .await
            {
                Ok(a) => a,
                Err(_) => continue,
            };
            for artifact in &artifacts {
                let is_match = artifact
                    .source
                    .as_ref()
                    .and_then(|s| s.user.as_deref())
                    .map(|u| u == target_username)
                    .unwrap_or(false);
                if !is_match {
                    continue;
                }

                // Track contributed project.
                let key = (org.name.clone(), project.clone());
                project_set.insert(key.clone());
                *project_release_counts.entry(key).or_default() += 1;

                // Collect for recent releases list.
                all_releases.push(RecentRelease {
                    org: org.name.clone(),
                    project: project.clone(),
                    slug: artifact.slug.clone(),
                    version: artifact.git_ref.as_ref().and_then(|r| r.version.clone()),
                    branch: artifact.git_ref.as_ref().and_then(|r| r.branch.clone()),
                    commit_sha: artifact.git_ref.as_ref().map(|r| r.commit_sha.clone()),
                    created_at: artifact.created_at.clone(),
                });

                // Heatmap day counts.
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&artifact.created_at) {
                    let date = dt.date_naive();
                    if date >= grid_start && date <= today {
                        *day_counts.entry(date).or_default() += 1;
                        total_contributions += 1;
                    }
                }
            }
        }
    }

    // Sort releases by created_at descending, take top 10.
    all_releases.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    all_releases.truncate(10);
    let recent_releases: Vec<minijinja::Value> = all_releases
        .into_iter()
        .map(|r| {
            context! {
                org => r.org,
                project => r.project,
                slug => r.slug,
                version => r.version,
                branch => r.branch,
                commit_sha => r.commit_sha.as_deref().map(|s| &s[..s.len().min(7)]),
                created_at => r.created_at,
            }
        })
        .collect();

    // Build contributed projects list sorted by release count descending.
    let mut contributed_projects: Vec<((String, String), u32)> = project_release_counts
        .into_iter()
        .collect();
    contributed_projects.sort_by(|a, b| b.1.cmp(&a.1));
    let contributed_projects: Vec<minijinja::Value> = contributed_projects
        .into_iter()
        .map(|((org, project), count)| {
            context! {
                org => org,
                project => project,
                release_count => count,
            }
        })
        .collect();

    // Build the grid: 53 columns (weeks) x 7 rows (days).
    // Use a power scale (sqrt) to map counts to 5 levels (0-4).
    let max_count = day_counts.values().copied().max().unwrap_or(0);

    let mut weeks: Vec<minijinja::Value> = Vec::new();
    let mut current = grid_start;
    let grid_end = grid_start + chrono::Duration::days(53 * 7 - 1);

    let mut week_days: Vec<minijinja::Value> = Vec::new();
    while current <= grid_end && current <= today {
        let count = day_counts.get(&current).copied().unwrap_or(0);
        let opacity = contribution_opacity(count, max_count);
        let in_range = current >= grid_start && current <= today;

        week_days.push(context! {
            date => current.format("%Y-%m-%d").to_string(),
            count => count,
            opacity => opacity,
            in_range => in_range,
        });

        // End of week (Saturday) — flush to weeks.
        if current.weekday() == chrono::Weekday::Sat || current == today {
            weeks.push(minijinja::Value::from_serialize(&week_days));
            week_days = Vec::new();
        }
        current += chrono::Duration::days(1);
    }
    if !week_days.is_empty() {
        weeks.push(minijinja::Value::from_serialize(&week_days));
    }

    // Month labels: first occurrence of each month in the grid.
    let mut month_labels: Vec<minijinja::Value> = Vec::new();
    let mut last_month = None;
    let mut col = 0usize;
    let mut d = grid_start;
    while d <= today {
        if d.weekday() == chrono::Weekday::Sun {
            let m = d.month();
            if last_month != Some(m) {
                last_month = Some(m);
                let label = d.format("%b").to_string();
                month_labels.push(context! { col => col, label => label });
            }
            col += 1;
        }
        d += chrono::Duration::days(1);
    }

    let heatmap = context! {
        weeks => weeks,
        month_labels => month_labels,
        total => total_contributions,
        max_count => max_count,
    };

    UserProfileData {
        heatmap,
        recent_releases,
        contributed_projects,
    }
}

/// Map a contribution count to an opacity 0.0–1.0 using a power scale (sqrt).
/// This gives a smooth gradient where low counts still have visible color
/// and high counts don't dwarf everything else.
fn contribution_opacity(count: u32, max_count: u32) -> String {
    if count == 0 || max_count == 0 {
        return "0".to_string();
    }
    // sqrt scale: brings low values up, compresses high values.
    // Min opacity 0.15 so even 1 contribution is visible.
    let ratio = (count as f64).sqrt() / (max_count as f64).sqrt();
    let opacity = 0.15 + ratio * 0.85;
    format!("{:.2}", opacity.clamp(0.15, 1.0))
}

// ─── Timeline builder (shared between dashboard, project detail, releases) ───

struct ArtifactWithProject {
    artifact: forage_core::platform::Artifact,
    project_name: String,
}

struct TimelineData {
    timeline: Vec<minijinja::Value>,
    lanes: Vec<minijinja::Value>,
}

/// Pipeline info indexed by project name, for overlaying onto releases.
type PipelinesByProject = std::collections::HashMap<String, Vec<forage_core::platform::ReleasePipeline>>;



/// Topologically sort pipeline run stage states by their `depends_on` edges.
fn topo_sort_run_stages(
    stages: &[forage_core::platform::PipelineRunStageState],
) -> Vec<&forage_core::platform::PipelineRunStageState> {
    use std::collections::{HashMap, VecDeque};

    let index_by_id: HashMap<&str, usize> = stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.stage_id.as_str(), i))
        .collect();

    let mut in_degree = vec![0u32; stages.len()];
    for (i, stage) in stages.iter().enumerate() {
        for dep in &stage.depends_on {
            if index_by_id.contains_key(dep.as_str()) {
                in_degree[i] += 1;
            }
        }
    }

    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, stage) in stages.iter().enumerate() {
        for dep in &stage.depends_on {
            if let Some(&dep_idx) = index_by_id.get(dep.as_str()) {
                dependents.entry(dep_idx).or_default().push(i);
            }
        }
    }

    let mut queue: VecDeque<usize> = in_degree
        .iter()
        .enumerate()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| i)
        .collect();

    let mut result = Vec::with_capacity(stages.len());
    while let Some(idx) = queue.pop_front() {
        result.push(&stages[idx]);
        if let Some(deps) = dependents.get(&idx) {
            for &dep_idx in deps {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push_back(dep_idx);
                }
            }
        }
    }

    if result.len() < stages.len() {
        let in_result: std::collections::HashSet<usize> =
            result.iter().map(|s| index_by_id[s.stage_id.as_str()]).collect();
        for (i, stage) in stages.iter().enumerate() {
            if !in_result.contains(&i) {
                result.push(stage);
            }
        }
    }

    result
}

fn build_timeline(
    items: Vec<ArtifactWithProject>,
    org_name: &str,
    environments: &[forage_core::platform::Environment],
    deployment_states: &forage_core::platform::DeploymentStates,
    release_intents: &[forage_core::platform::ReleaseIntentState],
    pipelines_by_project: &PipelinesByProject,
) -> TimelineData {
    // Index destination states by artifact_id for quick lookup.
    let mut states_by_artifact: std::collections::HashMap<
        &str,
        Vec<&forage_core::platform::DestinationState>,
    > = std::collections::HashMap::new();
    for ds in &deployment_states.destinations {
        if let Some(aid) = ds.artifact_id.as_deref() {
            states_by_artifact.entry(aid).or_default().push(ds);
        }
    }

    // Index release intent stages by artifact_id for quick lookup.
    let mut intent_stages_by_artifact: std::collections::HashMap<
        &str,
        &[forage_core::platform::PipelineRunStageState],
    > = std::collections::HashMap::new();
    for ri in release_intents {
        if !ri.stages.is_empty() {
            intent_stages_by_artifact.insert(ri.artifact_id.as_str(), &ri.stages);
        }
    }

    struct RawRelease {
        value: minijinja::Value,
        has_dests: bool,
    }

    let mut raw_releases: Vec<RawRelease> = Vec::new();

    for item in items {
        let artifact = item.artifact;
        let project = &item.project_name;

        // Look up deployment state from destination states instead of artifact.destinations.
        let matching_states = states_by_artifact
            .get(artifact.artifact_id.as_str())
            .cloned()
            .unwrap_or_default();

        let mut release_envs = Vec::new();
        let mut release_env_statuses = Vec::new();
        let dests: Vec<minijinja::Value> = matching_states
            .iter()
            .map(|ds| {
                release_envs.push(ds.environment.clone());
                let status_str = ds.status.as_deref().unwrap_or("PENDING");
                release_env_statuses.push(format!("{}:{}", ds.environment, status_str));
                context! {
                    name => ds.destination_name,
                    environment => ds.environment,
                    status => ds.status,
                    error_message => ds.error_message,
                    queued_at => ds.queued_at,
                    started_at => ds.started_at,
                    completed_at => ds.completed_at,
                    queue_position => ds.queue_position,
                }
            })
            .collect();

        let has_dests = !dests.is_empty();
        let dest_envs_str = release_env_statuses.join(",");
        let mut seen_envs = std::collections::HashSet::new();
        let unique_envs: Vec<String> = release_envs
            .iter()
            .filter(|e| seen_envs.insert(e.as_str()))
            .cloned()
            .collect();

        // Group environments by status for the summary line.
        // Each env gets its best (highest-priority) status.
        let mut env_best_status: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for ds in &matching_states {
            let status = ds.status.as_deref().unwrap_or("PENDING");
            let env = ds.environment.as_str();
            let current = env_best_status.get(env).copied().unwrap_or("PENDING");
            let priority = |s: &str| -> u8 {
                match s {
                    "RUNNING" | "ASSIGNED" => 6,
                    "QUEUED" => 5,
                    "FAILED" => 4,
                    "TIMED_OUT" => 3,
                    "CANCELLED" => 2,
                    "SUCCEEDED" => 1,
                    _ => 0,
                }
            };
            if priority(status) > priority(current) {
                env_best_status.insert(env, status);
            }
        }
        // Build groups sorted by priority (deploying first), then collect envs per group.
        let status_order = [
            "RUNNING", "QUEUED", "FAILED", "TIMED_OUT", "CANCELLED", "SUCCEEDED",
        ];
        let mut env_groups: Vec<minijinja::Value> = Vec::new();
        for &group_status in &status_order {
            let envs_in_group: Vec<String> = unique_envs
                .iter()
                .filter(|e| env_best_status.get(e.as_str()).copied() == Some(group_status))
                .cloned()
                .collect();
            if !envs_in_group.is_empty() {
                // Normalize ASSIGNED to RUNNING for display
                let display_status = if group_status == "ASSIGNED" {
                    "RUNNING"
                } else {
                    group_status
                };
                env_groups.push(context! {
                    status => display_status,
                    envs => envs_in_group,
                });
            }
        }

        // Build pipeline stage view from pipeline run data (if available) or
        // fall back to heuristic matching from destination states.
        let pipeline_stages: Vec<minijinja::Value> = {
            let mut stages = Vec::new();

            // First, check if the server returned pipeline run data for this artifact.
            if let Some(run_stages) = intent_stages_by_artifact.get(artifact.artifact_id.as_str()) {
                let sorted = topo_sort_run_stages(run_stages);
                for rs in sorted {
                    let wait_until_str = rs.wait_until.as_deref();
                    // For deploy stages the orchestrator may mark a stage as
                    // RUNNING before the actual destinations have started.
                    // Check destination states: if all destinations for this
                    // environment are still QUEUED, report the stage as QUEUED.
                    let display_status = if rs.stage_type == "deploy"
                        && (rs.status == "RUNNING" || rs.status == "ASSIGNED")
                    {
                        if let Some(ref env) = rs.environment {
                            let env_dests: Vec<&str> = matching_states
                                .iter()
                                .filter(|ds| ds.environment == *env)
                                .filter_map(|ds| ds.status.as_deref())
                                .collect();
                            if !env_dests.is_empty()
                                && env_dests.iter().all(|s| *s == "QUEUED")
                            {
                                "QUEUED"
                            } else {
                                &rs.status
                            }
                        } else {
                            &rs.status
                        }
                    } else {
                        &rs.status
                    };
                    stages.push(context! {
                        id => rs.stage_id,
                        stage_type => rs.stage_type,
                        environment => rs.environment,
                        duration_seconds => rs.duration_seconds,
                        depends_on => rs.depends_on,
                        status => display_status,
                        started_at => rs.started_at,
                        completed_at => rs.completed_at,
                        error_message => rs.error_message,
                        wait_until => wait_until_str,
                    });
                }
            }
            // No heuristic fallback: if there is no pipeline run data for
            // this artifact we leave pipeline_stages empty.  The frontend
            // uses env_groups to decide between "Deployed" and "Queued".
            stages
        };
        // A release "has a pipeline" if we have stage data from the server,
        // OR if the project has an enabled pipeline config (for not-yet-deployed releases).
        let project_has_enabled_pipeline = pipelines_by_project
            .get(project)
            .map(|ps| ps.iter().any(|p| p.enabled))
            .unwrap_or(false);
        let has_pipeline = !pipeline_stages.is_empty() || project_has_enabled_pipeline;

        // Compute summary status from individual destination statuses.
        // Priority: RUNNING/ASSIGNED > QUEUED > FAILED/TIMED_OUT/CANCELLED > SUCCEEDED
        let summary_status = if !has_dests {
            "PENDING"
        } else {
            let statuses: Vec<&str> = matching_states
                .iter()
                .filter_map(|ds| ds.status.as_deref())
                .collect();
            if statuses.iter().any(|s| *s == "RUNNING" || *s == "ASSIGNED") {
                "RUNNING"
            } else if statuses.contains(&"QUEUED") {
                "QUEUED"
            } else if statuses.contains(&"FAILED") {
                "FAILED"
            } else if statuses.contains(&"TIMED_OUT") {
                "TIMED_OUT"
            } else if statuses.contains(&"CANCELLED") {
                "CANCELLED"
            } else if statuses.contains(&"SUCCEEDED") {
                "SUCCEEDED"
            } else {
                "PENDING"
            }
        };

        raw_releases.push(RawRelease {
            value: context! {
                artifact_id => artifact.artifact_id,
                slug => artifact.slug,
                title => artifact.context.title,
                description => artifact.context.description,
                web => artifact.context.web,
                pr => artifact.context.pr,
                project_name => project,
                org_name => org_name,
                created_at => artifact.created_at,
                commit_sha => artifact.git_ref.as_ref().map(|r| r.commit_sha.clone()),
                branch => artifact.git_ref.as_ref().and_then(|r| r.branch.clone()),
                version => artifact.git_ref.as_ref().and_then(|r| r.version.clone()),
                commit_message => artifact.git_ref.as_ref().and_then(|r| r.commit_message.clone()),
                repo_url => artifact.git_ref.as_ref().and_then(|r| r.repo_url.clone()),
                source_user => artifact.source.as_ref().and_then(|s| s.user.clone()),
                source_email => artifact.source.as_ref().and_then(|s| s.email.clone()),
                source_type => artifact.source.as_ref().and_then(|s| s.source_type.clone()),
                run_url => artifact.source.as_ref().and_then(|s| s.run_url.clone()),
                destinations => dests,
                dest_envs => dest_envs_str,
                unique_envs => unique_envs,
                env_groups => env_groups,
                summary_status => summary_status,
                pipeline_stages => pipeline_stages,
                has_pipeline => has_pipeline,
            },
            has_dests,
        });
    }

    // Use environments from the API (sorted by sort_order), falling back to
    // environments discovered from destination states.
    let lanes: Vec<minijinja::Value> = if !environments.is_empty() {
        let mut envs: Vec<_> = environments.to_vec();
        envs.sort_by_key(|e| e.sort_order);
        envs.iter()
            .map(|env| {
                context! {
                    name => env.name,
                    description => env.description,
                    color => env_lane_color(&env.name),
                }
            })
            .collect()
    } else {
        let mut env_set = std::collections::BTreeSet::new();
        for ds in &deployment_states.destinations {
            if !ds.environment.is_empty() {
                env_set.insert(ds.environment.clone());
            }
        }
        env_set
            .into_iter()
            .map(|env| {
                let color = env_lane_color(&env);
                context! { name => env, color => color }
            })
            .collect()
    };

    // Truncate: keep everything up to the last deployed release, plus 3
    // older items for context.
    let last_deployed_idx = raw_releases
        .iter()
        .rposition(|r| r.has_dests)
        .map(|i| i + 1)
        .unwrap_or(0);
    let keep = last_deployed_idx + 3;
    if keep < raw_releases.len() {
        raw_releases.truncate(keep);
    }

    let mut timeline_items: Vec<minijinja::Value> = Vec::new();
    let mut hidden_buf: Vec<minijinja::Value> = Vec::new();
    let mut seen_deployed = false;

    for raw in raw_releases {
        if raw.has_dests {
            // Flush any hidden buffer before a deployed release
            if !hidden_buf.is_empty() {
                let count = hidden_buf.len();
                timeline_items.push(context! {
                    kind => "hidden",
                    count => count,
                    releases => std::mem::take(&mut hidden_buf),
                });
            }
            seen_deployed = true;
            timeline_items.push(context! {
                kind => "release",
                release => raw.value,
            });
        } else if !seen_deployed {
            // Before any deployment: show as regular (pending) release
            timeline_items.push(context! {
                kind => "release",
                release => raw.value,
            });
        } else {
            // After a deployment: group as hidden
            hidden_buf.push(raw.value);
        }
    }
    if !hidden_buf.is_empty() {
        let count = hidden_buf.len();
        timeline_items.push(context! {
            kind => "hidden",
            count => count,
            releases => std::mem::take(&mut hidden_buf),
        });
    }

    TimelineData {
        timeline: timeline_items,
        lanes,
    }
}

// ─── Serialisable API types (for the JSON timeline endpoint) ─────────

#[derive(Debug, Serialize)]
pub struct ApiTimelineResponse {
    pub timeline: Vec<ApiTimelineItem>,
    pub lanes: Vec<ApiLane>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiTimelineItem {
    Release { release: Box<ApiRelease> },
    Hidden { count: usize, releases: Vec<ApiRelease> },
}

#[derive(Debug, Serialize)]
pub struct ApiRelease {
    pub artifact_id: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_intent_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub web: Option<String>,
    pub pr: Option<String>,
    pub project_name: String,
    pub created_at: String,
    pub commit_sha: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
    pub commit_message: Option<String>,
    pub repo_url: Option<String>,
    pub source_user: Option<String>,
    pub source_type: Option<String>,
    pub run_url: Option<String>,
    pub summary_status: String,
    pub has_pipeline: bool,
    pub dest_envs: String,
    pub destinations: Vec<ApiDestinationState>,
    pub env_groups: Vec<ApiEnvGroup>,
    pub pipeline_stages: Vec<ApiPipelineStage>,
}

#[derive(Debug, Serialize)]
pub struct ApiDestinationState {
    pub name: String,
    pub environment: String,
    pub status: Option<String>,
    pub error_message: Option<String>,
    pub queued_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub queue_position: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ApiEnvGroup {
    pub status: String,
    pub envs: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiPipelineStage {
    pub id: String,
    pub stage_type: String,
    pub environment: Option<String>,
    pub duration_seconds: Option<i64>,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub wait_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_approve: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ApiLane {
    pub name: String,
    pub color: String,
    pub description: Option<String>,
}

/// Build a serialisable timeline from the same inputs as `build_timeline`.
/// The logic is kept intentionally parallel so both renderers stay in sync.
fn build_timeline_json(
    items: Vec<ArtifactWithProject>,
    environments: &[forage_core::platform::Environment],
    deployment_states: &forage_core::platform::DeploymentStates,
    release_intents: &[forage_core::platform::ReleaseIntentState],
    pipelines_by_project: &PipelinesByProject,
    approval_envs: &[String],
) -> ApiTimelineResponse {
    // Index destination states by artifact_id.
    let mut states_by_artifact: std::collections::HashMap<
        &str,
        Vec<&forage_core::platform::DestinationState>,
    > = std::collections::HashMap::new();
    for ds in &deployment_states.destinations {
        if let Some(aid) = ds.artifact_id.as_deref() {
            states_by_artifact.entry(aid).or_default().push(ds);
        }
    }

    // Index pipeline run stages and intent IDs by artifact_id.
    let mut intent_stages_by_artifact: std::collections::HashMap<
        &str,
        &[forage_core::platform::PipelineRunStageState],
    > = std::collections::HashMap::new();
    let mut intent_id_by_artifact: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    for ri in release_intents {
        if !ri.stages.is_empty() {
            intent_stages_by_artifact.insert(ri.artifact_id.as_str(), &ri.stages);
            intent_id_by_artifact.insert(ri.artifact_id.as_str(), ri.release_intent_id.as_str());
        }
    }

    struct RawRelease {
        release: ApiRelease,
        has_dests: bool,
    }

    let priority = |s: &str| -> u8 {
        match s {
            "RUNNING" | "ASSIGNED" => 6,
            "QUEUED" => 5,
            "FAILED" => 4,
            "TIMED_OUT" => 3,
            "CANCELLED" => 2,
            "SUCCEEDED" => 1,
            _ => 0,
        }
    };

    let mut raw_releases: Vec<RawRelease> = Vec::new();

    for item in items {
        let artifact = item.artifact;
        let project = item.project_name;

        let matching_states = states_by_artifact
            .get(artifact.artifact_id.as_str())
            .cloned()
            .unwrap_or_default();

        let mut release_envs: Vec<String> = Vec::new();
        let mut release_env_statuses: Vec<String> = Vec::new();
        let destinations: Vec<ApiDestinationState> = matching_states
            .iter()
            .map(|ds| {
                release_envs.push(ds.environment.clone());
                let status_str = ds.status.as_deref().unwrap_or("PENDING");
                release_env_statuses.push(format!("{}:{}", ds.environment, status_str));
                ApiDestinationState {
                    name: ds.destination_name.clone(),
                    environment: ds.environment.clone(),
                    status: ds.status.clone(),
                    error_message: ds.error_message.clone(),
                    queued_at: ds.queued_at.clone(),
                    started_at: ds.started_at.clone(),
                    completed_at: ds.completed_at.clone(),
                    queue_position: ds.queue_position,
                }
            })
            .collect();

        let has_dests = !destinations.is_empty();
        let dest_envs = release_env_statuses.join(",");

        let mut seen_envs = std::collections::HashSet::new();
        let unique_envs: Vec<String> = release_envs
            .iter()
            .filter(|e| seen_envs.insert(e.as_str()))
            .cloned()
            .collect();

        // Per-environment best status for grouping.
        let mut env_best_status: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for ds in &matching_states {
            let status = ds.status.as_deref().unwrap_or("PENDING");
            let env = ds.environment.as_str();
            let current = env_best_status.get(env).copied().unwrap_or("PENDING");
            if priority(status) > priority(current) {
                env_best_status.insert(env, status);
            }
        }

        let status_order = [
            "RUNNING", "QUEUED", "FAILED", "TIMED_OUT", "CANCELLED", "SUCCEEDED",
        ];
        let env_groups: Vec<ApiEnvGroup> = status_order
            .iter()
            .filter_map(|&group_status| {
                let envs_in_group: Vec<String> = unique_envs
                    .iter()
                    .filter(|e| env_best_status.get(e.as_str()).copied() == Some(group_status))
                    .cloned()
                    .collect();
                if envs_in_group.is_empty() {
                    return None;
                }
                let display_status = if group_status == "ASSIGNED" {
                    "RUNNING"
                } else {
                    group_status
                };
                Some(ApiEnvGroup {
                    status: display_status.to_string(),
                    envs: envs_in_group,
                })
            })
            .collect();

        // Build pipeline stages — same logic as build_timeline.
        let pipeline_stages: Vec<ApiPipelineStage> = {
            let mut stages = Vec::new();

            if let Some(run_stages) = intent_stages_by_artifact.get(artifact.artifact_id.as_str()) {
                let sorted = topo_sort_run_stages(run_stages);
                for rs in sorted {
                    // Same destination-aware override as build_timeline.
                    let display_status = if rs.stage_type == "deploy"
                        && (rs.status == "RUNNING" || rs.status == "ASSIGNED")
                    {
                        if let Some(ref env) = rs.environment {
                            let env_dests: Vec<&str> = matching_states
                                .iter()
                                .filter(|ds| ds.environment == *env)
                                .filter_map(|ds| ds.status.as_deref())
                                .collect();
                            if !env_dests.is_empty()
                                && env_dests.iter().all(|s| *s == "QUEUED")
                            {
                                "QUEUED".to_string()
                            } else if rs.status == "ASSIGNED" {
                                "RUNNING".to_string()
                            } else {
                                rs.status.clone()
                            }
                        } else if rs.status == "ASSIGNED" {
                            "RUNNING".to_string()
                        } else {
                            rs.status.clone()
                        }
                    } else if rs.status == "ASSIGNED" {
                        "RUNNING".to_string()
                    } else {
                        rs.status.clone()
                    };
                    let blocked_by = if display_status == "PENDING"
                        && rs.stage_type == "deploy"
                        && rs.environment.as_deref().map(|e| approval_envs.iter().any(|a| a == e)).unwrap_or(false)
                    {
                        Some("Awaiting approval".into())
                    } else {
                        None
                    };
                    // For plan stages, use AWAITING_APPROVAL as display status when appropriate
                    let display_status = if rs.stage_type == "plan"
                        && rs.approval_status.as_deref() == Some("AWAITING_APPROVAL")
                    {
                        "AWAITING_APPROVAL".to_string()
                    } else {
                        display_status
                    };
                    stages.push(ApiPipelineStage {
                        id: rs.stage_id.clone(),
                        stage_type: rs.stage_type.clone(),
                        environment: rs.environment.clone(),
                        duration_seconds: rs.duration_seconds,
                        status: display_status,
                        started_at: rs.started_at.clone(),
                        completed_at: rs.completed_at.clone(),
                        error_message: rs.error_message.clone(),
                        wait_until: rs.wait_until.clone(),
                        blocked_by,
                        approval_status: rs.approval_status.clone(),
                        auto_approve: rs.auto_approve,
                    });
                }
            }
            // No heuristic fallback — same rationale as build_timeline.
            stages
        };

        let project_has_enabled_pipeline = pipelines_by_project
            .get(&project)
            .map(|ps| ps.iter().any(|p| p.enabled))
            .unwrap_or(false);
        let has_pipeline = !pipeline_stages.is_empty() || project_has_enabled_pipeline;

        let summary_status = if !has_dests {
            "PENDING"
        } else {
            let statuses: Vec<&str> = matching_states
                .iter()
                .filter_map(|ds| ds.status.as_deref())
                .collect();
            if statuses.iter().any(|s| *s == "RUNNING" || *s == "ASSIGNED") {
                "RUNNING"
            } else if statuses.contains(&"QUEUED") {
                "QUEUED"
            } else if statuses.contains(&"FAILED") {
                "FAILED"
            } else if statuses.contains(&"TIMED_OUT") {
                "TIMED_OUT"
            } else if statuses.contains(&"CANCELLED") {
                "CANCELLED"
            } else if statuses.contains(&"SUCCEEDED") {
                "SUCCEEDED"
            } else {
                "PENDING"
            }
        };

        raw_releases.push(RawRelease {
            release: ApiRelease {
                release_intent_id: intent_id_by_artifact
                    .get(artifact.artifact_id.as_str())
                    .map(|s| s.to_string()),
                artifact_id: artifact.artifact_id,
                slug: artifact.slug,
                title: artifact.context.title,
                description: artifact.context.description,
                web: artifact.context.web,
                pr: artifact.context.pr,
                project_name: project,
                created_at: artifact.created_at,
                commit_sha: artifact.git_ref.as_ref().map(|r| r.commit_sha.clone()),
                branch: artifact.git_ref.as_ref().and_then(|r| r.branch.clone()),
                version: artifact.git_ref.as_ref().and_then(|r| r.version.clone()),
                commit_message: artifact.git_ref.as_ref().and_then(|r| r.commit_message.clone()),
                repo_url: artifact.git_ref.as_ref().and_then(|r| r.repo_url.clone()),
                source_user: artifact.source.as_ref().and_then(|s| s.user.clone()),
                source_type: artifact.source.as_ref().and_then(|s| s.source_type.clone()),
                run_url: artifact.source.as_ref().and_then(|s| s.run_url.clone()),
                summary_status: summary_status.to_string(),
                has_pipeline,
                dest_envs,
                destinations,
                env_groups,
                pipeline_stages,
            },
            has_dests,
        });
    }

    // Build lanes — same logic as build_timeline.
    let lanes: Vec<ApiLane> = if !environments.is_empty() {
        let mut envs = environments.to_vec();
        envs.sort_by_key(|e| e.sort_order);
        envs.iter()
            .map(|env| ApiLane {
                name: env.name.clone(),
                color: env_lane_color(&env.name).to_string(),
                description: env.description.clone(),
            })
            .collect()
    } else {
        let mut env_set = std::collections::BTreeSet::new();
        for ds in &deployment_states.destinations {
            if !ds.environment.is_empty() {
                env_set.insert(ds.environment.clone());
            }
        }
        env_set
            .into_iter()
            .map(|env| ApiLane {
                color: env_lane_color(&env).to_string(),
                name: env,
                description: None,
            })
            .collect()
    };

    // Truncate: keep up to last deployed + 3.
    let last_deployed_idx = raw_releases
        .iter()
        .rposition(|r| r.has_dests)
        .map(|i| i + 1)
        .unwrap_or(0);
    let keep = last_deployed_idx + 3;
    if keep < raw_releases.len() {
        raw_releases.truncate(keep);
    }

    let mut timeline: Vec<ApiTimelineItem> = Vec::new();
    let mut hidden_buf: Vec<ApiRelease> = Vec::new();
    let mut seen_deployed = false;

    for raw in raw_releases {
        let needs_action = raw.release.pipeline_stages.iter().any(|s| {
            s.blocked_by.is_some()
                || (s.stage_type == "plan" && s.status == "AWAITING_APPROVAL")
        });
        if raw.has_dests || needs_action {
            if !hidden_buf.is_empty() {
                let count = hidden_buf.len();
                timeline.push(ApiTimelineItem::Hidden {
                    count,
                    releases: std::mem::take(&mut hidden_buf),
                });
            }
            if raw.has_dests {
                seen_deployed = true;
            }
            timeline.push(ApiTimelineItem::Release {
                release: Box::new(raw.release),
            });
        } else if !seen_deployed {
            timeline.push(ApiTimelineItem::Release {
                release: Box::new(raw.release),
            });
        } else {
            hidden_buf.push(raw.release);
        }
    }
    if !hidden_buf.is_empty() {
        let count = hidden_buf.len();
        timeline.push(ApiTimelineItem::Hidden {
            count,
            releases: std::mem::take(&mut hidden_buf),
        });
    }

    ApiTimelineResponse { timeline, lanes }
}

// ─── GET /api/orgs/{org}/projects/{project}/timeline ─────────────────

async fn timeline_api(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid project name" })),
        )
            .into_response());
    }

    let (artifacts, environments, dest_states, release_intents, project_pipelines, policies) = tokio::join!(
        state
            .platform_client
            .list_artifacts(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, Some(&project)),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, Some(&project), true),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
        state
            .platform_client
            .list_policies(&session.access_token, &org, &project),
    );
    let artifacts = artifacts.map_err(|e| {
        tracing::error!("timeline_api list_artifacts: {e:#}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to fetch artifacts" })),
        )
            .into_response()
    })?;
    let environments = warn_default("list_environments", environments);
    let dest_states = warn_default("get_destination_states", dest_states);
    let release_intents = warn_default("get_release_intent_states", release_intents);
    let project_pipelines = warn_default("list_release_pipelines", project_pipelines);

    let items: Vec<ArtifactWithProject> = artifacts
        .into_iter()
        .map(|a| ArtifactWithProject {
            artifact: a,
            project_name: project.clone(),
        })
        .collect();

    let mut pipelines_map = PipelinesByProject::new();
    if !project_pipelines.is_empty() {
        pipelines_map.insert(project.clone(), project_pipelines);
    }

    let policies = warn_default("list_policies", policies);

    let approval_envs: Vec<String> = policies
        .iter()
        .filter(|p| p.enabled && p.policy_type == "approval")
        .filter_map(|p| match &p.config {
            PolicyConfig::Approval { target_environment, .. } => Some(target_environment.clone()),
            _ => None,
        })
        .collect();

    let data = build_timeline_json(items, &environments, &dest_states, &release_intents, &pipelines_map, &approval_envs);

    Ok(Json(data).into_response())
}

// ─── GET /api/orgs/{org}/timeline ────────────────────────────────────

async fn org_timeline_api(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    let (projects, environments, dest_states, release_intents) = tokio::join!(
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, None),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, None, true),
    );
    let projects = projects.map_err(|e| {
        tracing::error!("org_timeline_api list_projects: {e:#}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to fetch projects" })),
        )
            .into_response()
    })?;
    let environments = warn_default("list_environments", environments);
    let dest_states = warn_default("get_destination_states", dest_states);
    let release_intents = warn_default("get_release_intent_states", release_intents);

    let mut pipelines_by_project = PipelinesByProject::new();
    for p in &projects {
        let pipelines = warn_default(
            "list_release_pipelines",
            state
                .platform_client
                .list_release_pipelines(&session.access_token, &org, p)
                .await,
        );
        if !pipelines.is_empty() {
            pipelines_by_project.insert(p.clone(), pipelines);
        }
    }

    let items = fetch_org_artifacts(&state, &session.access_token, &org, &projects).await;
    let data = build_timeline_json(
        items,
        &environments,
        &dest_states,
        &release_intents,
        &pipelines_by_project,
        &[], // org timeline doesn't have per-project policy context
    );

    Ok(Json(data).into_response())
}

/// Fetch all artifacts across projects and return as ArtifactWithProject list.
async fn fetch_org_artifacts(
    state: &AppState,
    access_token: &str,
    org: &str,
    projects: &[String],
) -> Vec<ArtifactWithProject> {
    let mut items = Vec::new();
    for project in projects {
        let artifacts = warn_default(
            &format!("list_artifacts({project})"),
            state.platform_client.list_artifacts(access_token, org, project).await,
        );
        for artifact in artifacts {
            items.push(ArtifactWithProject {
                artifact,
                project_name: project.clone(),
            });
        }
    }
    items
}

// ─── Releases (Up-inspired pipeline) ─────────────────────────────────

async fn releases_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    let (projects, environments, dest_states, release_intents) = tokio::join!(
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .get_destination_states(&session.access_token, &org, None),
        state
            .platform_client
            .get_release_intent_states(&session.access_token, &org, None, true),
    );
    let projects = projects.map_err(|e| internal_error(&state, "list_projects", &e))?;
    let environments = warn_default("list_environments", environments);
    let dest_states = warn_default("get_destination_states", dest_states);
    let release_intents = warn_default("get_release_intent_states", release_intents);

    // Fetch pipelines for all projects.
    let mut pipelines_by_project = PipelinesByProject::new();
    for p in &projects {
        let pipelines = warn_default(
            "list_release_pipelines",
            state
                .platform_client
                .list_release_pipelines(&session.access_token, &org, p)
                .await,
        );
        if !pipelines.is_empty() {
            pipelines_by_project.insert(p.clone(), pipelines);
        }
    }

    let items = fetch_org_artifacts(&state, &session.access_token, &org, &projects).await;
    let data = build_timeline(items, &org, &environments, &dest_states, &release_intents, &pipelines_by_project);

    let mut sorted_envs = environments.clone();
    sorted_envs.sort_by_key(|e| e.sort_order);
    let env_options: Vec<minijinja::Value> = sorted_envs
        .iter()
        .map(|e| context! { name => e.name })
        .collect();

    let html = state
        .templates
        .render(
            "pages/releases.html.jinja",
            context! {
                title => format!("Releases - {org} - Forage"),
                description => format!("Deployment pipeline for {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                timeline => data.timeline,
                lanes => data.lanes,
                env_options => env_options,
                active_tab => "releases",
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

// ─── Destinations ────────────────────────────────────────────────────

async fn destinations_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let (environments, org_destinations, projects, dest_types) = tokio::join!(
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .list_destinations(&session.access_token, &org),
        state
            .platform_client
            .list_projects(&session.access_token, &org),
        state
            .platform_client
            .list_destination_types(&session.access_token),
    );
    let mut environments = environments.map_err(|e| internal_error(&state, "list_environments", &e))?;
    environments.sort_by_key(|e| e.sort_order);
    let org_destinations = org_destinations.map_err(|e| internal_error(&state, "list_destinations", &e))?;
    let projects = warn_default("list_projects", projects);
    let dest_types = warn_default("list_destination_types", dest_types);
    let destination_types_json = serde_json::to_string(&dest_types).unwrap_or_else(|_| "[]".to_string());

    let destination_types: Vec<minijinja::Value> = dest_types
        .iter()
        .map(|t| {
            context! {
                organisation => &t.organisation,
                name => &t.name,
                version => t.version,
                description => &t.description,
            }
        })
        .collect();

    let env_list: Vec<minijinja::Value> = environments
        .iter()
        .map(|e| {
            let env_dests: Vec<minijinja::Value> = org_destinations
                .iter()
                .filter(|d| d.environment == e.name)
                .map(|d| {
                    let meta_entries: Vec<minijinja::Value> = d
                        .metadata
                        .iter()
                        .map(|(k, v)| context! { key => k, value => v })
                        .collect();
                    let metadata_json = serde_json::to_string(&d.metadata).unwrap_or_default();
                    context! {
                        name => d.name,
                        environment => d.environment,
                        type_name => d.dest_type.as_ref().map(|t| t.name.clone()),
                        type_org => d.dest_type.as_ref().map(|t| t.organisation.clone()),
                        type_version => d.dest_type.as_ref().map(|t| t.version),
                        metadata => meta_entries,
                        metadata_json => metadata_json,
                    }
                })
                .collect();
            context! {
                id => e.id,
                name => e.name,
                description => e.description,
                sort_order => e.sort_order,
                destinations => env_dests,
            }
        })
        .collect();

    // Also collect destinations not associated with any known environment
    let known_envs: std::collections::HashSet<&str> =
        environments.iter().map(|e| e.name.as_str()).collect();
    let orphan_dests: Vec<minijinja::Value> = org_destinations
        .iter()
        .filter(|d| !known_envs.contains(d.environment.as_str()))
        .map(|d| {
            let meta_entries: Vec<minijinja::Value> = d
                .metadata
                .iter()
                .map(|(k, v)| context! { key => k, value => v })
                .collect();
            let metadata_json = serde_json::to_string(&d.metadata).unwrap_or_default();
            context! {
                name => d.name,
                environment => d.environment,
                type_name => d.dest_type.as_ref().map(|t| t.name.clone()),
                type_org => d.dest_type.as_ref().map(|t| t.organisation.clone()),
                type_version => d.dest_type.as_ref().map(|t| t.version),
                metadata => meta_entries,
                metadata_json => metadata_json,
            }
        })
        .collect();

    let html = state
        .templates
        .render(
            "pages/destinations.html.jinja",
            context! {
                title => format!("Destinations - {org} - Forage"),
                description => format!("Deployment destinations for {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                environments => env_list,
                orphan_destinations => orphan_dests,
                projects => projects,
                is_admin => is_admin,
                active_tab => "destinations",
                destination_types => destination_types,
                destination_types_json => destination_types_json,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct CreateEnvironmentForm {
    _csrf: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    sort_order: i32,
}

async fn create_environment_submit(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CreateEnvironmentForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(&state, StatusCode::FORBIDDEN, "Invalid request", "CSRF validation failed. Please try again."));
    }

    if !validate_slug(&form.name) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid environment name",
            "Environment names must be lowercase alphanumeric with hyphens, max 64 chars.",
        ));
    }

    let description = if form.description.is_empty() {
        None
    } else {
        Some(form.description.as_str())
    };

    state
        .platform_client
        .create_environment(
            &session.access_token,
            &org,
            &form.name,
            description,
            form.sort_order,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "create environment error", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/destinations")).into_response())
}

#[derive(Deserialize)]
struct CreateDestinationForm {
    _csrf: String,
    name: String,
    environment: String,
    #[serde(default)]
    type_organisation: String,
    #[serde(default)]
    type_name: String,
    #[serde(default)]
    type_version: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    metadata_keys: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    metadata_values: Vec<String>,
}

/// HTML forms send a single value as a string, multiple values as a sequence.
/// This deserializer handles both cases.
fn deserialize_string_or_seq<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or sequence of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(vec![v])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(v) = seq.next_element::<String>()? {
                values.push(v);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

fn parse_metadata(keys: &[String], values: &[String]) -> std::collections::HashMap<String, String> {
    keys.iter()
        .zip(values.iter())
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

async fn create_destination_submit(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<CreateDestinationForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid CSRF token",
            "Please try again.",
        ));
    }

    if form.name.is_empty() || form.environment.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Destination name and environment are required.",
        ));
    }

    let metadata = parse_metadata(&form.metadata_keys, &form.metadata_values);
    let dest_type = if !form.type_name.trim().is_empty() {
        Some(forage_core::platform::DestinationType {
            organisation: if form.type_organisation.trim().is_empty() {
                org.clone()
            } else {
                form.type_organisation.trim().to_string()
            },
            name: form.type_name.trim().to_string(),
            version: form.type_version.unwrap_or(1),
        })
    } else {
        None
    };

    state
        .platform_client
        .create_destination(
            &session.access_token,
            &org,
            &form.name,
            &form.environment,
            &metadata,
            dest_type.as_ref(),
        )
        .await
        .map_err(|e| {
            internal_error(&state, "create destination error", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/destinations")).into_response())
}

#[derive(Deserialize)]
struct DestinationQuery {
    name: String,
}

async fn destination_detail(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<DestinationQuery>,
) -> Result<Response, Response> {
    let dest_name = &query.name;
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let destinations = state
        .platform_client
        .list_destinations(&session.access_token, &org)
        .await
        .map_err(|e| internal_error(&state, "list_destinations", &e))?;

    let dest = destinations
        .iter()
        .find(|d| d.name == *dest_name)
        .ok_or_else(|| {
            error_page(
                &state,
                StatusCode::NOT_FOUND,
                "Destination not found",
                &format!("No destination named '{dest_name}' was found."),
            )
        })?;

    let meta_entries: Vec<minijinja::Value> = dest
        .metadata
        .iter()
        .map(|(k, v)| context! { key => k, value => v })
        .collect();

    let html = state
        .templates
        .render(
            "pages/destination_detail.html.jinja",
            context! {
                title => format!("{} - Destinations - {} - Forage", dest_name, org),
                description => format!("Destination {} in {}", dest_name, org),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                is_admin => is_admin,
                active_tab => "destinations",
                dest_name => &dest.name,
                dest_environment => &dest.environment,
                dest_type_name => dest.dest_type.as_ref().map(|t| t.name.clone()),
                dest_type_organisation => dest.dest_type.as_ref().map(|t| t.organisation.clone()),
                dest_type_version => dest.dest_type.as_ref().map(|t| t.version),
                metadata => meta_entries,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct UpdateDestinationForm {
    _csrf: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    metadata_keys: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    metadata_values: Vec<String>,
}

async fn update_destination_submit(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<DestinationQuery>,
    Form(form): Form<UpdateDestinationForm>,
) -> Result<Response, Response> {
    let dest_name = &query.name;
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid CSRF token",
            "Please try again.",
        ));
    }

    let metadata = parse_metadata(&form.metadata_keys, &form.metadata_values);

    state
        .platform_client
        .update_destination(&session.access_token, dest_name, &metadata)
        .await
        .map_err(|e| {
            internal_error(&state, "update destination error", &e)
        })?;

    let encoded_name = urlencoding::encode(dest_name);
    Ok(
        Redirect::to(&format!(
            "/orgs/{org}/destinations/detail?name={encoded_name}"
        ))
        .into_response(),
    )
}

// ─── Members ────────────────────────────────────────────────────────

async fn members_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;

    let members = state
        .platform_client
        .list_members(&session.access_token, &current_org.organisation_id)
        .await
        .map_err(|e| internal_error(&state, "list_members", &e))?;

    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let html = state
        .templates
        .render(
            "pages/members.html.jinja",
            context! {
                title => format!("Members - {org} - Forage"),
                description => format!("Members of {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                is_admin => is_admin,
                active_tab => "settings",
                members => members.iter().map(|m| context! {
                    user_id => m.user_id,
                    username => m.username,
                    role => m.role,
                    joined_at => m.joined_at,
                }).collect::<Vec<_>>(),
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct AddMemberForm {
    username: String,
    role: String,
    _csrf: String,
}

async fn add_member_submit(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<AddMemberForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let _ = state
        .platform_client
        .add_member(
            &session.access_token,
            &current_org.organisation_id,
            &form.username,
            &form.role,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "failed to add member", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/settings/members")).into_response())
}

#[derive(Deserialize)]
struct UpdateRoleForm {
    role: String,
    _csrf: String,
}

async fn update_member_role_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, user_id)): Path<(String, String)>,
    Form(form): Form<UpdateRoleForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let _ = state
        .platform_client
        .update_member_role(
            &session.access_token,
            &current_org.organisation_id,
            &user_id,
            &form.role,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "failed to update member role", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/settings/members")).into_response())
}

#[derive(Deserialize)]
struct CsrfForm {
    _csrf: String,
}

async fn remove_member_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, user_id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    state
        .platform_client
        .remove_member(
            &session.access_token,
            &current_org.organisation_id,
            &user_id,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "failed to remove member", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/settings/members")).into_response())
}

// ─── Auto-Release Policies ──────────────────────────────────────────

// ─── Triggers (auto-release triggers) ───────────────────────────────

async fn triggers_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (triggers, environments, destinations, pipelines) = tokio::join!(
        state
            .platform_client
            .list_triggers(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .list_destinations(&session.access_token, &org),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
    );
    let triggers = triggers.map_err(|e| internal_error(&state, "list_triggers", &e))?;
    let environments = warn_default("list_environments", environments);
    let destinations = warn_default("list_destinations", destinations);
    let pipelines = warn_default("list_release_pipelines", pipelines);

    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let trigger_items: Vec<minijinja::Value> = triggers
        .iter()
        .map(|t| {
            context! {
                id => t.id,
                name => t.name,
                enabled => t.enabled,
                branch_pattern => t.branch_pattern,
                title_pattern => t.title_pattern,
                author_pattern => t.author_pattern,
                commit_message_pattern => t.commit_message_pattern,
                source_type_pattern => t.source_type_pattern,
                target_environments => &t.target_environments,
                target_destinations => &t.target_destinations,
                force_release => t.force_release,
                use_pipeline => t.use_pipeline,
                created_at => t.created_at,
                updated_at => t.updated_at,
            }
        })
        .collect();

    let env_options: Vec<minijinja::Value> = environments
        .iter()
        .map(|e| context! { name => e.name })
        .collect();

    let dest_options: Vec<minijinja::Value> = destinations
        .iter()
        .map(|d| context! { name => d.name, environment => d.environment })
        .collect();

    let pipeline_options: Vec<minijinja::Value> = pipelines
        .iter()
        .filter(|p| p.enabled)
        .map(|p| context! { name => p.name })
        .collect();

    let projects = warn_default(
        "list_projects",
        state.platform_client.list_projects(&session.access_token, &org).await,
    );

    let html = state
        .templates
        .render(
            "pages/triggers.html.jinja",
            context! {
                page_title => format!("Triggers · {} · {}", project, org),
                user => context! {
                    username => session.user.username,
                },
                csrf_token => session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => org,
                current_project => project,
                projects => projects,
                triggers => trigger_items,
                environments => env_options,
                destinations => dest_options,
                pipelines => pipeline_options,
                is_admin => is_admin,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct CreateTriggerForm {
    csrf_token: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    branch_pattern: String,
    #[serde(default)]
    title_pattern: String,
    #[serde(default)]
    author_pattern: String,
    #[serde(default)]
    commit_message_pattern: String,
    #[serde(default)]
    source_type_pattern: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    target_environments: Vec<String>,
    #[serde(default)]
    force_release: Option<String>,
    #[serde(default)]
    use_pipeline: Option<String>,
}

async fn create_trigger_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<CreateTriggerForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    if form.name.trim().is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Trigger name is required.",
        ));
    }

    // At least one filter pattern is required
    let has_pattern = non_empty(&form.branch_pattern).is_some()
        || non_empty(&form.title_pattern).is_some()
        || non_empty(&form.author_pattern).is_some()
        || non_empty(&form.commit_message_pattern).is_some()
        || non_empty(&form.source_type_pattern).is_some();
    if !has_pattern {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "At least one filter pattern is required.",
        ));
    }

    let environments: Vec<String> = form
        .target_environments
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if environments.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "At least one target environment is required.",
        ));
    }

    // Auto-generate name from first pattern if not provided
    let name = if form.name.trim().is_empty() {
        let pattern = non_empty(&form.branch_pattern)
            .or_else(|| non_empty(&form.title_pattern))
            .or_else(|| non_empty(&form.author_pattern))
            .or_else(|| non_empty(&form.commit_message_pattern))
            .or_else(|| non_empty(&form.source_type_pattern))
            .unwrap_or_default();
        let envs = environments.join("-");
        format!("{}-to-{}", pattern, envs)
    } else {
        form.name.trim().to_string()
    };

    let input = CreateTriggerInput {
        name,
        branch_pattern: non_empty(&form.branch_pattern),
        title_pattern: non_empty(&form.title_pattern),
        author_pattern: non_empty(&form.author_pattern),
        commit_message_pattern: non_empty(&form.commit_message_pattern),
        source_type_pattern: non_empty(&form.source_type_pattern),
        target_environments: environments,
        target_destinations: vec![],
        force_release: form.force_release.as_deref() == Some("true"),
        use_pipeline: form.use_pipeline.as_deref() == Some("true"),
    };

    state
        .platform_client
        .create_trigger(&session.access_token, &org, &project, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to create trigger", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/triggers")).into_response())
}

#[derive(Deserialize)]
struct ToggleTriggerForm {
    csrf_token: String,
    #[serde(default)]
    enabled: Option<String>,
}

async fn toggle_trigger(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<ToggleTriggerForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let input = UpdateTriggerInput {
        enabled: Some(form.enabled.is_some()),
        branch_pattern: None,
        title_pattern: None,
        author_pattern: None,
        commit_message_pattern: None,
        source_type_pattern: None,
        target_environments: vec![],
        target_destinations: vec![],
        force_release: None,
        use_pipeline: None,
    };

    state
        .platform_client
        .update_trigger(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to toggle trigger", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/triggers")).into_response())
}

#[derive(Deserialize)]
struct DeleteTriggerForm {
    csrf_token: String,
}

async fn delete_trigger(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<DeleteTriggerForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    state
        .platform_client
        .delete_trigger(&session.access_token, &org, &project, &name)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to delete trigger", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/triggers")).into_response())
}

async fn edit_trigger_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (triggers, environments, pipelines) = tokio::join!(
        state
            .platform_client
            .list_triggers(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
    );
    let triggers = triggers.map_err(|e| internal_error(&state, "list_triggers", &e))?;
    let environments = warn_default("list_environments", environments);
    let pipelines = warn_default("list_release_pipelines", pipelines);

    let trigger = triggers
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| {
            error_page(
                &state,
                StatusCode::NOT_FOUND,
                "Not found",
                "Trigger not found.",
            )
        })?;

    let trigger_ctx = context! {
        name => trigger.name,
        enabled => trigger.enabled,
        branch_pattern => trigger.branch_pattern,
        title_pattern => trigger.title_pattern,
        author_pattern => trigger.author_pattern,
        commit_message_pattern => trigger.commit_message_pattern,
        source_type_pattern => trigger.source_type_pattern,
        target_environments => &trigger.target_environments,
        target_destinations => &trigger.target_destinations,
        force_release => trigger.force_release,
        use_pipeline => trigger.use_pipeline,
    };

    let env_options: Vec<minijinja::Value> = environments
        .iter()
        .map(|e| context! { name => e.name })
        .collect();

    let pipeline_options: Vec<minijinja::Value> = pipelines
        .iter()
        .filter(|p| p.enabled)
        .map(|p| context! { name => p.name })
        .collect();

    let projects = warn_default(
        "list_projects",
        state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await,
    );

    let html = state
        .templates
        .render(
            "pages/trigger_edit.html.jinja",
            context! {
                page_title => format!("Edit Trigger · {} · {}", name, org),
                user => context! {
                    username => session.user.username,
                },
                csrf_token => session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => org,
                current_project => project,
                projects => projects,
                trigger => trigger_ctx,
                environments => env_options,
                pipelines => pipeline_options,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct EditTriggerForm {
    csrf_token: String,
    #[serde(default)]
    branch_pattern: String,
    #[serde(default)]
    title_pattern: String,
    #[serde(default)]
    author_pattern: String,
    #[serde(default)]
    commit_message_pattern: String,
    #[serde(default)]
    source_type_pattern: String,
    #[serde(default, deserialize_with = "deserialize_string_or_seq")]
    target_environments: Vec<String>,
    #[serde(default)]
    force_release: Option<String>,
    #[serde(default)]
    use_pipeline: Option<String>,
}

async fn edit_trigger_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<EditTriggerForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let has_pattern = non_empty(&form.branch_pattern).is_some()
        || non_empty(&form.title_pattern).is_some()
        || non_empty(&form.author_pattern).is_some()
        || non_empty(&form.commit_message_pattern).is_some()
        || non_empty(&form.source_type_pattern).is_some();
    if !has_pattern {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "At least one filter pattern is required.",
        ));
    }

    let environments: Vec<String> = form
        .target_environments
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if environments.is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "At least one target environment is required.",
        ));
    }

    let input = UpdateTriggerInput {
        enabled: None,
        branch_pattern: non_empty(&form.branch_pattern),
        title_pattern: non_empty(&form.title_pattern),
        author_pattern: non_empty(&form.author_pattern),
        commit_message_pattern: non_empty(&form.commit_message_pattern),
        source_type_pattern: non_empty(&form.source_type_pattern),
        target_environments: environments,
        target_destinations: vec![],
        force_release: Some(form.force_release.as_deref() == Some("true")),
        use_pipeline: Some(form.use_pipeline.as_deref() == Some("true")),
    };

    state
        .platform_client
        .update_trigger(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| internal_error(&state, "failed to update trigger", &e))?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/triggers")).into_response())
}

// ─── Policies (deployment gating) ──────────────────────────────────

async fn policies_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (policies, environments) = tokio::join!(
        state
            .platform_client
            .list_policies(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
    );
    let policies = policies.map_err(|e| internal_error(&state, "list_policies", &e))?;
    let environments = warn_default("list_environments", environments);

    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let policy_items: Vec<minijinja::Value> = policies
        .iter()
        .map(|p| {
            let (policy_type, config_detail) = match &p.config {
                PolicyConfig::SoakTime {
                    source_environment,
                    target_environment,
                    duration_seconds,
                } => (
                    "soak_time",
                    context! {
                        source_environment => source_environment,
                        target_environment => target_environment,
                        duration_seconds => duration_seconds,
                        duration_human => format_duration(*duration_seconds),
                    },
                ),
                PolicyConfig::BranchRestriction {
                    target_environment,
                    branch_pattern,
                } => (
                    "branch_restriction",
                    context! {
                        target_environment => target_environment,
                        branch_pattern => branch_pattern,
                    },
                ),
                PolicyConfig::Approval {
                    target_environment,
                    required_approvals,
                } => (
                    "approval",
                    context! {
                        target_environment => target_environment,
                        required_approvals => required_approvals,
                    },
                ),
            };
            context! {
                id => p.id,
                name => p.name,
                enabled => p.enabled,
                policy_type => policy_type,
                config => config_detail,
                created_at => p.created_at,
                updated_at => p.updated_at,
            }
        })
        .collect();

    let env_options: Vec<minijinja::Value> = environments
        .iter()
        .map(|e| context! { name => e.name })
        .collect();

    let projects = warn_default(
        "list_projects",
        state.platform_client.list_projects(&session.access_token, &org).await,
    );

    let html = state
        .templates
        .render(
            "pages/policies.html.jinja",
            context! {
                page_title => format!("Policies · {} · {}", project, org),
                user => context! {
                    username => session.user.username,
                },
                csrf_token => session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => org,
                current_project => project,
                projects => projects,
                policies => policy_items,
                environments => env_options,
                is_admin => is_admin,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

/// Map environment name to a swimlane bar color (matches ENV_COLORS in swim-lanes.js).
fn env_lane_color(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("prod") && !lower.contains("preprod") && !lower.contains("pre-prod") {
        "#ec4899"
    } else if lower.contains("preprod") || lower.contains("pre-prod") {
        "#f97316"
    } else if lower.contains("stag") {
        "#eab308"
    } else if lower.contains("dev") {
        "#8b5cf6"
    } else if lower.contains("test") {
        "#06b6d4"
    } else {
        "#6b7280"
    }
}

fn format_duration(seconds: i64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}s", seconds)
    }
}

#[derive(Deserialize)]
struct CreatePolicyForm {
    csrf_token: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    policy_type: String,
    // SoakTime fields
    #[serde(default)]
    source_environment: String,
    #[serde(default)]
    target_environment: String,
    #[serde(default)]
    duration_seconds: Option<i64>,
    // BranchRestriction fields
    #[serde(default)]
    branch_pattern: String,
    // Approval fields
    #[serde(default)]
    required_approvals: Option<i32>,
}

async fn create_policy_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<CreatePolicyForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    if form.name.trim().is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Policy name is required.",
        ));
    }

    let config = match form.policy_type.as_str() {
        "soak_time" => {
            let source = form.source_environment.trim();
            let target = form.target_environment.trim();
            let duration = form.duration_seconds.unwrap_or(0);
            if source.is_empty() || target.is_empty() || duration <= 0 {
                return Err(error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    "Invalid request",
                    "Soak time requires source environment, target environment, and a positive duration.",
                ));
            }
            PolicyConfig::SoakTime {
                source_environment: source.to_string(),
                target_environment: target.to_string(),
                duration_seconds: duration,
            }
        }
        "branch_restriction" => {
            let target = form.target_environment.trim();
            let pattern = form.branch_pattern.trim();
            if target.is_empty() || pattern.is_empty() {
                return Err(error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    "Invalid request",
                    "Branch restriction requires a target environment and branch pattern.",
                ));
            }
            PolicyConfig::BranchRestriction {
                target_environment: target.to_string(),
                branch_pattern: pattern.to_string(),
            }
        }
        "approval" => {
            let target = form.target_environment.trim();
            let required = form.required_approvals.unwrap_or(1);
            if target.is_empty() || required < 1 {
                return Err(error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    "Invalid request",
                    "Approval requires a target environment and at least 1 required approval.",
                ));
            }
            PolicyConfig::Approval {
                target_environment: target.to_string(),
                required_approvals: required,
            }
        }
        _ => {
            return Err(error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "Invalid request",
                "Invalid policy type.",
            ));
        }
    };

    let input = CreatePolicyInput {
        name: form.name.trim().to_string(),
        config,
    };

    state
        .platform_client
        .create_policy(&session.access_token, &org, &project, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to create policy", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/policies")).into_response())
}

#[derive(Deserialize)]
struct TogglePolicyForm {
    csrf_token: String,
    #[serde(default)]
    enabled: Option<String>,
}

async fn toggle_policy(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<TogglePolicyForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let input = UpdatePolicyInput {
        enabled: Some(form.enabled.is_some()),
        config: None,
    };

    state
        .platform_client
        .update_policy(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to toggle policy", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/policies")).into_response())
}

#[derive(Deserialize)]
struct DeletePolicyForm {
    csrf_token: String,
}

async fn delete_policy(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<DeletePolicyForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    state
        .platform_client
        .delete_policy(&session.access_token, &org, &project, &name)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to delete policy", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/policies")).into_response())
}

async fn edit_policy_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (policies, environments) = tokio::join!(
        state
            .platform_client
            .list_policies(&session.access_token, &org, &project),
        state
            .platform_client
            .list_environments(&session.access_token, &org),
    );
    let policies = policies.map_err(|e| internal_error(&state, "list_policies", &e))?;
    let environments = warn_default("list_environments", environments);

    let policy = policies
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            error_page(
                &state,
                StatusCode::NOT_FOUND,
                "Not found",
                "Policy not found.",
            )
        })?;

    let (policy_type, config_ctx) = match &policy.config {
        PolicyConfig::SoakTime {
            source_environment,
            target_environment,
            duration_seconds,
        } => (
            "soak_time",
            context! {
                source_environment => source_environment,
                target_environment => target_environment,
                duration_seconds => duration_seconds,
            },
        ),
        PolicyConfig::BranchRestriction {
            target_environment,
            branch_pattern,
        } => (
            "branch_restriction",
            context! {
                target_environment => target_environment,
                branch_pattern => branch_pattern,
            },
        ),
        PolicyConfig::Approval {
            target_environment,
            required_approvals,
        } => (
            "approval",
            context! {
                target_environment => target_environment,
                required_approvals => required_approvals,
            },
        ),
    };

    let policy_ctx = context! {
        name => policy.name,
        enabled => policy.enabled,
        policy_type => policy_type,
        config => config_ctx,
    };

    let env_options: Vec<minijinja::Value> = environments
        .iter()
        .map(|e| context! { name => e.name })
        .collect();

    let projects = warn_default(
        "list_projects",
        state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await,
    );

    let html = state
        .templates
        .render(
            "pages/policy_edit.html.jinja",
            context! {
                page_title => format!("Edit Policy · {} · {}", name, org),
                user => context! {
                    username => session.user.username,
                },
                csrf_token => session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => org,
                current_project => project,
                projects => projects,
                policy => policy_ctx,
                environments => env_options,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct EditPolicyForm {
    csrf_token: String,
    #[serde(default)]
    policy_type: String,
    #[serde(default)]
    source_environment: String,
    #[serde(default)]
    target_environment: String,
    #[serde(default)]
    duration_seconds: Option<i64>,
    #[serde(default)]
    branch_pattern: String,
}

async fn edit_policy_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<EditPolicyForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if form.csrf_token != session.csrf_token {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let config = match form.policy_type.as_str() {
        "soak_time" => {
            let source = form.source_environment.trim();
            let target = form.target_environment.trim();
            let duration = form.duration_seconds.unwrap_or(0);
            if source.is_empty() || target.is_empty() || duration <= 0 {
                return Err(error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    "Invalid request",
                    "Soak time requires source environment, target environment, and a positive duration.",
                ));
            }
            PolicyConfig::SoakTime {
                source_environment: source.to_string(),
                target_environment: target.to_string(),
                duration_seconds: duration,
            }
        }
        "branch_restriction" => {
            let target = form.target_environment.trim();
            let pattern = form.branch_pattern.trim();
            if target.is_empty() || pattern.is_empty() {
                return Err(error_page(
                    &state,
                    StatusCode::BAD_REQUEST,
                    "Invalid request",
                    "Branch restriction requires a target environment and branch pattern.",
                ));
            }
            PolicyConfig::BranchRestriction {
                target_environment: target.to_string(),
                branch_pattern: pattern.to_string(),
            }
        }
        _ => {
            return Err(error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "Invalid request",
                "Invalid policy type.",
            ));
        }
    };

    let input = UpdatePolicyInput {
        enabled: None,
        config: Some(config),
    };

    state
        .platform_client
        .update_policy(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| internal_error(&state, "failed to update policy", &e))?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/policies")).into_response())
}

// ─── Release Pipelines ──────────────────────────────────────────────

#[tracing::instrument(skip(state, session), fields(org, project))]
async fn pipelines_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let (pipelines, projects) = tokio::join!(
        state
            .platform_client
            .list_release_pipelines(&session.access_token, &org, &project),
        state
            .platform_client
            .list_projects(&session.access_token, &org),
    );
    let pipelines = pipelines.map_err(|e| internal_error(&state, "list_pipelines", &e))?;
    let projects = warn_default("list_projects", projects);

    let is_admin = current_org.role == "owner" || current_org.role == "admin";

    let pipeline_items: Vec<minijinja::Value> = pipelines
        .iter()
        .map(|p| {
            let stage_count = p.stages.len();
            context! {
                id => p.id,
                name => p.name,
                enabled => p.enabled,
                stages_json => serde_json::to_string(&p.stages).unwrap_or_default(),
                stage_count => stage_count,
                created_at => p.created_at,
                updated_at => p.updated_at,
            }
        })
        .collect();

    let html = state
        .templates
        .render(
            "pages/pipelines.html.jinja",
            context! {
                page_title => format!("Pipelines · {} · {}", project, org),
                user => context! {
                    username => session.user.username,
                },
                csrf_token => session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => org,
                current_project => project,
                projects => projects,
                pipelines => pipeline_items,
                is_admin => is_admin,
            },
        )
        .map_err(|e| {
            internal_error(&state, "template error", &e)
        })?;

    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct CreatePipelineForm {
    _csrf: String,
    name: String,
    #[serde(default)]
    stages_json: String,
}

#[tracing::instrument(skip(state, session, form), fields(org, project))]
async fn create_pipeline_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<CreatePipelineForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    if form.name.trim().is_empty() {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Pipeline name is required.",
        ));
    }

    let stages: Vec<PipelineStage> = if form.stages_json.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&form.stages_json).map_err(|_| {
            error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "Invalid request",
                "stages_json is not valid JSON.",
            )
        })?
    };

    let input = CreateReleasePipelineInput {
        name: form.name.trim().to_string(),
        stages,
    };

    state
        .platform_client
        .create_release_pipeline(&session.access_token, &org, &project, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to create pipeline", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/pipelines")).into_response())
}

#[derive(Deserialize)]
struct TogglePipelineForm {
    _csrf: String,
    #[serde(default)]
    enabled: Option<String>,
}

#[tracing::instrument(skip(state, session, form), fields(org, project, name))]
async fn toggle_pipeline(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<TogglePipelineForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let input = UpdateReleasePipelineInput {
        enabled: Some(form.enabled.is_some()),
        stages: None,
    };

    state
        .platform_client
        .update_release_pipeline(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to toggle pipeline", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/pipelines")).into_response())
}

#[derive(Deserialize)]
struct UpdatePipelineForm {
    _csrf: String,
    #[serde(default)]
    stages_json: String,
}

#[tracing::instrument(skip(state, session, form), fields(org, project, name))]
async fn update_pipeline_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<UpdatePipelineForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let stages: Vec<PipelineStage> = if form.stages_json.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&form.stages_json).map_err(|_| {
            error_page(
                &state,
                StatusCode::BAD_REQUEST,
                "Invalid request",
                "stages_json is not valid JSON.",
            )
        })?
    };

    let input = UpdateReleasePipelineInput {
        enabled: None,
        stages: Some(stages),
    };

    state
        .platform_client
        .update_release_pipeline(&session.access_token, &org, &project, &name, &input)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to update pipeline", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/pipelines")).into_response())
}

#[derive(Deserialize)]
struct DeletePipelineForm {
    _csrf: String,
}

#[tracing::instrument(skip(state, session, form), fields(org, project, name))]
async fn delete_pipeline(
    State(state): State<AppState>,
    session: Session,
    Path((org, project, name)): Path<(String, String, String)>,
    Form(form): Form<DeletePipelineForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let current_org = require_org_membership(&state, orgs, &org)?;
    require_admin(&state, current_org)?;

    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    state
        .platform_client
        .delete_release_pipeline(&session.access_token, &org, &project, &name)
        .await
        .map_err(|e| {
            internal_error(&state, "failed to delete pipeline", &e)
        })?;

    Ok(Redirect::to(&format!("/orgs/{org}/projects/{project}/pipelines")).into_response())
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ── Approval routes ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApprovalForm {
    csrf_token: String,
    #[serde(default)]
    release_intent_id: String,
    #[serde(default)]
    target_environment: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    force_bypass: Option<String>,
}

fn approval_error(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    status: StatusCode,
    message: &str,
) -> Response {
    let wants_json = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"));

    if wants_json {
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    } else {
        error_page(state, status, "Approval failed", message)
    }
}

async fn approve_release_submit(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    Path((org, project, slug)): Path<(String, String, String)>,
    Form(form): Form<ApprovalForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if form.csrf_token != session.csrf_token {
        return Err(approval_error(
            &state,
            &headers,
            StatusCode::FORBIDDEN,
            "CSRF validation failed. Please try again.",
        ));
    }

    let force_bypass = form.force_bypass.as_deref() == Some("true");
    let comment = non_empty(&form.comment);

    state
        .platform_client
        .approve_release(
            &session.access_token,
            &org,
            &project,
            &form.release_intent_id,
            &form.target_environment,
            comment.as_deref(),
            force_bypass,
        )
        .await
        .map_err(|e| match e {
            forage_core::platform::PlatformError::NotAuthenticated => {
                axum::response::Redirect::to("/login").into_response()
            }
            other => approval_error(
                &state,
                &headers,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("{other}"),
            ),
        })?;

    Ok(Redirect::to(&format!(
        "/orgs/{org}/projects/{project}/releases/{slug}"
    ))
    .into_response())
}

async fn reject_release_submit(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    Path((org, project, slug)): Path<(String, String, String)>,
    Form(form): Form<ApprovalForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if form.csrf_token != session.csrf_token {
        return Err(approval_error(
            &state,
            &headers,
            StatusCode::FORBIDDEN,
            "CSRF validation failed. Please try again.",
        ));
    }

    let comment = non_empty(&form.comment);

    state
        .platform_client
        .reject_release(
            &session.access_token,
            &org,
            &project,
            &form.release_intent_id,
            &form.target_environment,
            comment.as_deref(),
        )
        .await
        .map_err(|e| match e {
            forage_core::platform::PlatformError::NotAuthenticated => {
                axum::response::Redirect::to("/login").into_response()
            }
            other => approval_error(
                &state,
                &headers,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("{other}"),
            ),
        })?;

    Ok(Redirect::to(&format!(
        "/orgs/{org}/projects/{project}/releases/{slug}"
    ))
    .into_response())
}

// ── Plan stage approve / reject / output ─────────────────────────────

#[derive(Deserialize)]
struct PlanStageForm {
    csrf_token: String,
    release_intent_id: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    redirect_to: Option<String>,
}

async fn approve_plan_stage_submit(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    Path((org, _project, stage_id)): Path<(String, String, String)>,
    Form(form): Form<PlanStageForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if form.csrf_token != session.csrf_token {
        return Err(approval_error(
            &state,
            &headers,
            StatusCode::FORBIDDEN,
            "CSRF validation failed. Please try again.",
        ));
    }

    state
        .platform_client
        .approve_plan_stage(
            &session.access_token,
            &form.release_intent_id,
            &stage_id,
        )
        .await
        .map_err(|e| match e {
            forage_core::platform::PlatformError::NotAuthenticated => {
                axum::response::Redirect::to("/login").into_response()
            }
            other => approval_error(
                &state,
                &headers,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("{other}"),
            ),
        })?;

    if let Some(redirect) = &form.redirect_to {
        Ok(Redirect::to(redirect).into_response())
    } else {
        Ok(Json(serde_json::json!({ "ok": true })).into_response())
    }
}

async fn reject_plan_stage_submit(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    Path((org, _project, stage_id)): Path<(String, String, String)>,
    Form(form): Form<PlanStageForm>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if form.csrf_token != session.csrf_token {
        return Err(approval_error(
            &state,
            &headers,
            StatusCode::FORBIDDEN,
            "CSRF validation failed. Please try again.",
        ));
    }

    let reason = form.reason.as_deref().and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    });

    state
        .platform_client
        .reject_plan_stage(
            &session.access_token,
            &form.release_intent_id,
            &stage_id,
            reason.as_deref(),
        )
        .await
        .map_err(|e| match e {
            forage_core::platform::PlatformError::NotAuthenticated => {
                axum::response::Redirect::to("/login").into_response()
            }
            other => approval_error(
                &state,
                &headers,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("{other}"),
            ),
        })?;

    if let Some(redirect) = &form.redirect_to {
        Ok(Redirect::to(redirect).into_response())
    } else {
        Ok(Json(serde_json::json!({ "ok": true })).into_response())
    }
}

#[derive(Deserialize)]
struct PlanOutputQuery {
    release_intent_id: String,
}

async fn get_plan_output_api(
    State(state): State<AppState>,
    session: Session,
    Path((org, _project, stage_id)): Path<(String, String, String)>,
    Query(query): Query<PlanOutputQuery>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    let output = state
        .platform_client
        .get_plan_output(
            &session.access_token,
            &query.release_intent_id,
            &stage_id,
        )
        .await
        .map_err(|e| {
            internal_error(&state, "get plan output", &e)
        })?;

    let outputs: Vec<serde_json::Value> = output.outputs.iter().map(|o| {
        serde_json::json!({
            "destination_id": o.destination_id,
            "destination_name": o.destination_name,
            "plan_output": o.plan_output,
            "status": o.status,
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "plan_output": output.plan_output,
        "status": output.status,
        "outputs": outputs,
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Compute
// ---------------------------------------------------------------------------

async fn compute_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let _cached_org = require_org_membership(&state, orgs, &org)?;

    let (instances, rollouts) = if let Some(ref scheduler) = state.compute_scheduler {
        let namespace = &org;
        let instances = scheduler
            .list_instances(namespace)
            .await
            .unwrap_or_default();
        let rollouts = scheduler
            .list_rollouts(namespace)
            .await
            .unwrap_or_default();
        (instances, rollouts)
    } else {
        (vec![], vec![])
    };

    let instances_ctx: Vec<minijinja::Value> = instances
        .iter()
        .map(|i| {
            context! {
                id => i.id,
                resource_name => i.resource_name,
                project => i.project,
                destination => i.destination,
                environment => i.environment,
                image => i.image,
                region => i.region,
                replicas => i.replicas,
                cpu => i.cpu,
                memory => i.memory,
                status => i.status,
            }
        })
        .collect();

    let rollouts_ctx: Vec<minijinja::Value> = rollouts
        .iter()
        .take(20)
        .map(|r| {
            let resources: Vec<minijinja::Value> = r
                .resources
                .iter()
                .map(|res| {
                    context! {
                        name => res.name,
                        kind => res.kind.to_string(),
                        status => res.status.to_string(),
                        message => res.message,
                    }
                })
                .collect();
            context! {
                id => r.id,
                apply_id => r.apply_id,
                namespace => r.namespace,
                status => r.status.to_string(),
                resources => resources,
            }
        })
        .collect();

    let projects = warn_default(
        "compute: list projects",
        state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await,
    );

    let html = state
        .templates
        .render(
            "pages/compute.html.jinja",
            context! {
                title => format!("Compute - {} - Forage", org),
                description => "Managed compute instances",
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => &org,
                active_tab => "settings",
                projects => projects,
                instances => instances_ctx,
                rollouts => rollouts_ctx,
                org_name => &org,
            },
        )
        .map_err(|e| internal_error(&state, "compute render", &e))?;

    Ok(Html(html).into_response())
}

async fn rollout_detail_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, rollout_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    let _cached_org = require_org_membership(&state, orgs, &org)?;

    let scheduler = state.compute_scheduler.as_ref().ok_or_else(|| {
        error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Not available",
            "Compute is not enabled.",
        )
    })?;

    let rollout = scheduler.get_rollout(&rollout_id).await.map_err(|_| {
        error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Not found",
            "Rollout not found.",
        )
    })?;

    let resources_ctx: Vec<minijinja::Value> = rollout
        .resources
        .iter()
        .map(|r| {
            context! {
                name => r.name,
                kind => r.kind.to_string(),
                status => r.status.to_string(),
                message => r.message,
            }
        })
        .collect();

    let labels_ctx: Vec<minijinja::Value> = rollout.labels.iter().map(|(k, v)| context! { key => k, value => v }).collect();

    let rollout_ctx = context! {
        id => rollout.id,
        apply_id => rollout.apply_id,
        namespace => rollout.namespace,
        status => rollout.status.to_string(),
        resources => resources_ctx,
        labels => labels_ctx,
    };

    let projects = warn_default(
        "rollout detail: list projects",
        state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await,
    );

    let html = state
        .templates
        .render(
            "pages/rollout_detail.html.jinja",
            context! {
                title => format!("Rollout {} - Forage", rollout.apply_id),
                description => "Rollout details",
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                orgs => orgs_context(orgs),
                current_org => &org,
                active_tab => "settings",
                projects => projects,
                rollout => rollout_ctx,
                org_name => &org,
            },
        )
        .map_err(|e| internal_error(&state, "rollout detail render", &e))?;

    Ok(Html(html).into_response())
}

async fn regions_api() -> impl IntoResponse {
    let regions: Vec<serde_json::Value> = forage_core::compute::REGIONS
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "name": r.name,
                "display_name": r.display_name,
                "available": r.available,
            })
        })
        .collect();

    Json(regions)
}
