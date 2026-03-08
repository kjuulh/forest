use anyhow::Context;
use forest_grpc_interface::{
    release_service_server::ReleaseService, PipelineStageUpdate, *,
};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Response;

use crate::{
    actor::Actor,
    grpc::artifacts::GrpcErrorExt,
    services::{
        policy::{PolicyRegistryState, PolicyType},
        trigger::{AnnotationMatchData, TriggerRegistryState},
        event_bus::{EventBusState, EventPayload},
        notification_registry::{NotificationRegistryState, ReleaseContext as NotifReleaseContext},
        release_event_store::ReleaseEventStoreState,
        release_logs_registry::{LogChannel, ReleaseLogsRegistryState},
        release_pipeline::ReleasePipelineRegistryState,
        release_registry::{self, ReleaseAnnotation, ReleaseDestination, ReleaseRegistryState},
        users::UserServiceState,
    },
    state::State,
};

pub struct ReleaseServer {
    pub state: State,
}

#[async_trait::async_trait]
impl ReleaseService for ReleaseServer {
    async fn annotate_release(
        &self,
        request: tonic::Request<AnnotateReleaseRequest>,
    ) -> std::result::Result<tonic::Response<AnnotateReleaseResponse>, tonic::Status> {
        tracing::debug!("annotate release");

        let actor = request
            .extensions()
            .get::<Actor>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))?;

        let req = request.into_inner();

        let slug = petname::petname(3, "-").expect("to be able to generate slug");

        let proj = req
            .project
            .context("no project found")
            .to_internal_error()?;

        let artifact_id = req
            .artifact_id
            .parse::<uuid::Uuid>()
            .context("artifact id")
            .to_internal_error()?;

        // Extract source/context/ref info for both the annotate call and notification context
        let mut source: release_registry::Source = req
            .source
            .map(|s| s.into())
            .context("source is required")
            .to_internal_error()?;

        // For human users, always resolve identity from the DB.
        // Apps (CI systems etc.) are allowed to pass along third-party user info.
        if let Actor::User { user_id } = &actor {
            let user = self
                .state
                .user_service()
                .get_user(*user_id)
                .await
                .to_internal_error()?
                .ok_or_else(|| tonic::Status::internal("authenticated user not found in DB"))?;
            source.username = Some(user.username);
            source.email = user.emails.into_iter().next().map(|e| e.email);
        }
        let art_context: release_registry::ArtifactContext = req
            .context
            .map(|s| s.into())
            .context("context is required")
            .to_internal_error()?;
        let reference: release_registry::Reference = req
            .r#ref
            .map(|r| r.into())
            .context("ref is required")
            .to_internal_error()?;

        let artifact = self
            .state
            .release_registry()
            .annotate(
                &artifact_id,
                &slug,
                &req.metadata,
                &source,
                &art_context,
                &proj.organisation,
                &proj.project,
                &reference,
                &actor,
            )
            .await
            .to_internal_error()?;

        if let Err(e) = self
            .state
            .notification_registry()
            .create_notification(
                "RELEASE_ANNOTATED",
                &format!("Artifact annotated: {}", slug),
                &format!(
                    "Artifact {} annotated in {}/{}",
                    slug, &proj.organisation, &proj.project
                ),
                &proj.organisation,
                &proj.project,
                &NotifReleaseContext {
                    slug: Some(slug.clone()),
                    artifact_id: Some(artifact_id.to_string()),
                    source_username: source.username.clone(),
                    source_email: source.email.clone(),
                    source_type: source.source_type.clone(),
                    run_url: source.run_url.clone(),
                    commit_sha: Some(reference.commit_sha.clone()),
                    commit_branch: reference.commit_branch.clone(),
                    commit_message: reference.commit_message.clone(),
                    version: reference.version.clone(),
                    repo_url: reference.repo_url.clone(),
                    context_title: Some(art_context.title.clone()),
                    context_description: art_context.description.clone(),
                    context_web: art_context.web.clone(),
                    context_pr: art_context.pr.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!("failed to create annotation notification: {e:#}");
        }

        self.state.event_bus().emit(EventPayload {
            organisation: proj.organisation.clone(),
            project: proj.project.clone(),
            resource_type: "artifact",
            action: "created",
            resource_id: artifact_id.to_string(),
            metadata: [("slug".into(), slug.clone())].into(),
        }).await;

        // Evaluate triggers
        let match_data =
            AnnotationMatchData::from_parts(&source, &art_context, &reference);

        tracing::debug!(
            branch = ?match_data.branch,
            title = %match_data.title,
            "evaluating triggers for annotation"
        );

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&proj.organisation, &proj.project)
            .await;

        let project_id = match project_id {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("failed to resolve project for trigger evaluation: {e:#}");
                return Ok(Response::new(AnnotateReleaseResponse {
                    artifact: Some(artifact.into()),
                }));
            }
        };

        match self
            .state
            .trigger_registry()
            .evaluate(&project_id, &match_data)
            .await
        {
            Ok(trigger_matches) => {
                tracing::debug!(count = trigger_matches.len(), "triggers evaluated");
                for trigger_match in trigger_matches {
                    // Evaluate branch restriction policies for each target environment
                    let branch = reference.commit_branch.as_deref();
                    let mut blocked = false;
                    for env in &trigger_match.target_environments {
                        let evals = self
                            .state
                            .policy_registry()
                            .evaluate_for_environment(&project_id, env, branch)
                            .await
                            .unwrap_or_default();
                        for eval in &evals {
                            if !eval.passed
                                && eval.policy_type == PolicyType::BranchRestriction
                            {
                                tracing::info!(
                                    trigger = %trigger_match.trigger_name,
                                    policy = %eval.policy_name,
                                    env = %env,
                                    "trigger blocked by policy: {}",
                                    eval.reason,
                                );
                                blocked = true;
                                break;
                            }
                        }
                        if blocked {
                            break;
                        }
                    }
                    if blocked {
                        continue;
                    }

                    tracing::info!(
                        trigger = %trigger_match.trigger_name,
                        org = %proj.organisation,
                        project = %proj.project,
                        "trigger matched, triggering release"
                    );

                    match self
                        .state
                        .release_registry()
                        .release(
                            &artifact_id,
                            trigger_match.target_destinations,
                            trigger_match.target_environments,
                            &actor,
                            &self.state.release_event_store(),
                            trigger_match.force_release,
                            trigger_match.use_pipeline,
                            &self.state.release_pipeline_registry(),
                        )
                        .await
                    {
                        Ok(created) => {
                            tracing::info!(
                                trigger = %trigger_match.trigger_name,
                                intent_id = %created.release_intent_id,
                                destinations = created.releases.len(),
                                "trigger fired successfully"
                            );
                            // Signal the IntentCoordinator for pipeline releases
                            if trigger_match.use_pipeline {
                                let _ = self
                                    .state
                                    .nats
                                    .publish(
                                        "forest.intent.evaluate",
                                        created.release_intent_id.to_string().into(),
                                    )
                                    .await;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                trigger = %trigger_match.trigger_name,
                                "trigger release failed: {e:#}"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to evaluate triggers: {e:#}");
            }
        }

        Ok(Response::new(AnnotateReleaseResponse {
            artifact: Some(artifact.into()),
        }))
    }

    async fn get_artifact_by_slug(
        &self,
        request: tonic::Request<GetArtifactBySlugRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactBySlugResponse>, tonic::Status> {
        tracing::debug!("get artifact by slug");
        let req = request.into_inner();

        let release_annotation = self
            .state
            .release_registry()
            .get_release_annotation_by_slug(&req.slug)
            .await
            .context("get release annotation by slug")
            .to_internal_error()?;

        Ok(Response::new(GetArtifactBySlugResponse {
            artifact: Some(release_annotation.into()),
        }))
    }
    async fn get_artifacts_by_project(
        &self,
        request: tonic::Request<GetArtifactsByProjectRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactsByProjectResponse>, tonic::Status> {
        tracing::debug!("get artifact by project");
        let req = request.into_inner();

        let project = req
            .project
            .ok_or(anyhow::anyhow!("project is required"))
            .to_internal_error()?;

        let release_annotation = self
            .state
            .release_registry()
            .get_release_annotation_by_project(&project.organisation, &project.project)
            .await
            .context("get release annotation by project")
            .to_internal_error()?;

        Ok(Response::new(GetArtifactsByProjectResponse {
            artifact: release_annotation.into_iter().map(|r| r.into()).collect(),
        }))
    }

    async fn release(
        &self,
        request: tonic::Request<ReleaseRequest>,
    ) -> std::result::Result<tonic::Response<ReleaseResponse>, tonic::Status> {
        tracing::debug!("release");

        let actor = request
            .extensions()
            .get::<Actor>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))?;

        let req = request.into_inner();

        let artifact_id: uuid::Uuid = req
            .artifact_id
            .parse()
            .context("artifact id")
            .to_internal_error()?;

        // Evaluate branch restriction policies before releasing
        let ann_ctx_for_policy = self
            .state
            .release_registry()
            .get_annotation_context(&artifact_id)
            .await
            .ok();
        let branch_for_policy = ann_ctx_for_policy
            .as_ref()
            .and_then(|a| a.reference.commit_branch.clone());

        // Collect all target environments to check policies against
        let target_envs: Vec<String> = if !req.environments.is_empty() {
            req.environments.clone()
        } else {
            // When releasing to specific destinations, resolve their environments
            Vec::new()
        };

        if let Ok(project_id) = self
            .state
            .release_registry()
            .get_project_id_from_artifact(&artifact_id)
            .await
        {
            for env in &target_envs {
                let evaluations = self
                    .state
                    .policy_registry()
                    .evaluate_for_environment(&project_id, env, branch_for_policy.as_deref())
                    .await
                    .unwrap_or_default();

                for eval in &evaluations {
                    // Only enforce branch_restriction at request time.
                    // soak_time is handled by the scheduler (deferred retry).
                    if !eval.passed
                        && eval.policy_type == PolicyType::BranchRestriction
                    {
                        return Err(tonic::Status::failed_precondition(format!(
                            "blocked by policy '{}': {}",
                            eval.policy_name, eval.reason
                        )));
                    }
                }
            }
        }

        let created = self
            .state
            .release_registry()
            .release(
                &artifact_id,
                req.destinations,
                req.environments,
                &actor,
                &self.state.release_event_store(),
                req.force,
                req.use_pipeline,
                &self.state.release_pipeline_registry(),
            )
            .await
            .context("release")
            .to_internal_error()?;

        let dest_count = created.releases.len();
        let dest_names: Vec<String> = created
            .releases
            .iter()
            .map(|r| r.destination.clone())
            .collect();

        // Fetch annotation context to enrich the started notification
        let ann_ctx = self
            .state
            .release_registry()
            .get_annotation_context(&artifact_id)
            .await
            .ok();

        if let Err(e) = self
            .state
            .notification_registry()
            .create_notification(
                "RELEASE_STARTED",
                &format!(
                    "Release started: {}/{}",
                    &created.organisation, &created.project
                ),
                &format!("Release staged to {} destination(s)", dest_count),
                &created.organisation,
                &created.project,
                &NotifReleaseContext {
                    slug: ann_ctx.as_ref().map(|a| a.slug.clone()),
                    artifact_id: Some(artifact_id.to_string()),
                    release_intent_id: Some(created.release_intent_id.to_string()),
                    destination: if dest_names.len() == 1 {
                        Some(dest_names[0].clone())
                    } else {
                        None
                    },
                    destination_count: dest_count as i32,
                    source_username: ann_ctx.as_ref().and_then(|a| a.source.username.clone()),
                    source_email: ann_ctx.as_ref().and_then(|a| a.source.email.clone()),
                    source_type: ann_ctx
                        .as_ref()
                        .and_then(|a| a.source.source_type.clone()),
                    run_url: ann_ctx.as_ref().and_then(|a| a.source.run_url.clone()),
                    commit_sha: ann_ctx.as_ref().map(|a| a.reference.commit_sha.clone()),
                    commit_branch: ann_ctx
                        .as_ref()
                        .and_then(|a| a.reference.commit_branch.clone()),
                    commit_message: ann_ctx
                        .as_ref()
                        .and_then(|a| a.reference.commit_message.clone()),
                    version: ann_ctx
                        .as_ref()
                        .and_then(|a| a.reference.version.clone()),
                    repo_url: ann_ctx
                        .as_ref()
                        .and_then(|a| a.reference.repo_url.clone()),
                    context_title: ann_ctx.as_ref().map(|a| a.context.title.clone()),
                    context_description: ann_ctx
                        .as_ref()
                        .and_then(|a| a.context.description.clone()),
                    context_web: ann_ctx.as_ref().and_then(|a| a.context.web.clone()),
                    context_pr: ann_ctx.as_ref().and_then(|a| a.context.pr.clone()),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!("failed to create release started notification: {e:#}");
        }

        self.state.event_bus().emit(EventPayload {
            organisation: created.organisation.clone(),
            project: created.project.clone(),
            resource_type: "release",
            action: "created",
            resource_id: created.release_intent_id.to_string(),
            metadata: [
                ("destinations".into(), dest_names.join(",")),
            ].into(),
        }).await;

        // Signal the IntentCoordinator to evaluate this pipeline
        if !created.activated_stages.is_empty() || req.use_pipeline {
            let _ = self
                .state
                .nats
                .publish(
                    "forest.intent.evaluate",
                    created.release_intent_id.to_string().into(),
                )
                .await;
        }

        Ok(Response::new(ReleaseResponse {
            intents: created
                .releases
                .into_iter()
                .map(|r| ReleaseIntent {
                    release_intent_id: created.release_intent_id.to_string(),
                    destination: r.destination,
                    environment: r.environment,
                })
                .collect(),
        }))
    }

    type WaitReleaseStream = ReceiverStream<Result<WaitReleaseEvent, tonic::Status>>;

    async fn wait_release(
        &self,
        request: tonic::Request<WaitReleaseRequest>,
    ) -> std::result::Result<tonic::Response<Self::WaitReleaseStream>, tonic::Status> {
        tracing::debug!("wait_release stream");
        let req = request.into_inner();

        let release_intent_id: uuid::Uuid = req
            .release_intent_id
            .parse()
            .context("release_intent_id")
            .to_internal_error()?;

        let (tx, rx) = mpsc::channel(32);
        let release_registry = self.state.release_registry();
        let logs_registry = self.state.release_logs_registry();
        let db = self.state.db.clone();

        let nats = self.state.nats.clone();

        // Spawn a task that subscribes to NATS status changes and streams updates
        tokio::spawn(async move {
            // Subscribe to status changes for this intent
            let nats_subject = format!("forest.release.status.{}", release_intent_id);
            let mut nats_sub = match nats.subscribe(nats_subject).await {
                Ok(sub) => sub,
                Err(e) => {
                    let _ = tx
                        .send(Err(tonic::Status::internal(format!(
                            "failed to subscribe to NATS: {e}"
                        ))))
                        .await;
                    return;
                }
            };

            let mut last_statuses: std::collections::HashMap<
                uuid::Uuid,
                forest_models::ReleaseStatus,
            > = std::collections::HashMap::new();
            let mut log_cursors: std::collections::HashMap<uuid::Uuid, i64> =
                std::collections::HashMap::new();
            // Track last-seen stage statuses for pipeline stage updates
            let mut last_stage_statuses: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            let mut fallback_interval =
                tokio::time::interval(std::time::Duration::from_secs(2));
            fallback_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                // Wait for either a NATS notification or the fallback timer
                tokio::select! {
                    _msg = nats_sub.next() => {}
                    _ = fallback_interval.tick() => {}
                }

                // Stream pipeline stage updates (if this is a pipeline release)
                if let Ok(Some(intent_row)) = sqlx::query!(
                    "SELECT stages, stage_states FROM release_intents WHERE id = $1",
                    release_intent_id,
                )
                .fetch_optional(&db)
                .await
                {
                    if let (Some(stages_json), Some(stage_states_json)) =
                        (&intent_row.stages, &intent_row.stage_states)
                    {
                        use crate::services::release_pipeline::{
                            PipelineStages, StageConfig, StageStates,
                        };

                        if let (Ok(stages), Ok(stage_states)) = (
                            serde_json::from_value::<PipelineStages>(stages_json.clone()),
                            serde_json::from_value::<StageStates>(stage_states_json.clone()),
                        ) {
                            for (stage_id, state) in &stage_states {
                                let status_str = format!("{:?}", state.status).to_uppercase();
                                let changed = last_stage_statuses
                                    .get(stage_id)
                                    .map_or(true, |prev| *prev != status_str);

                                if changed {
                                    last_stage_statuses
                                        .insert(stage_id.clone(), status_str.clone());

                                    let stage_type = stages.get(stage_id).map(|def| {
                                        match &def.config {
                                            StageConfig::Deploy { .. } => "deploy",
                                            StageConfig::Wait { .. } => "wait",
                                        }
                                    }).unwrap_or("unknown");

                                    let event = WaitReleaseEvent {
                                        event: Some(
                                            wait_release_event::Event::StageUpdate(
                                                PipelineStageUpdate {
                                                    stage_id: stage_id.clone(),
                                                    stage_type: stage_type.to_string(),
                                                    status: status_str,
                                                    queued_at: state.queued_at.clone(),
                                                    started_at: state.started_at.clone(),
                                                    completed_at: state.completed_at.clone(),
                                                    wait_until: state.wait_until.clone(),
                                                    error_message: state.error_message.clone(),
                                                },
                                            ),
                                        ),
                                    };

                                    if tx.send(Ok(event)).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                // Fetch current state and stream updates
                match release_registry
                    .get_release_status_by_intent(&release_intent_id)
                    .await
                {
                    Ok(status_infos) => {
                        if status_infos.is_empty() {
                            // For pipeline releases with no deploy steps yet (e.g. wait stage),
                            // check if pipeline is fully complete via stage_states
                            if !last_stage_statuses.is_empty() {
                                let all_stages_terminal = last_stage_statuses.values().all(|s| {
                                    matches!(
                                        s.as_str(),
                                        "SUCCEEDED" | "FAILED" | "CANCELLED"
                                    )
                                });
                                if all_stages_terminal {
                                    break;
                                }
                            }
                            continue;
                        }

                        let mut all_finalized = true;

                        for status_info in &status_infos {
                            let status_changed = last_statuses
                                .get(&status_info.destination_id)
                                .map(|prev| *prev != status_info.status)
                                .unwrap_or(true);

                            if status_changed {
                                last_statuses
                                    .insert(status_info.destination_id, status_info.status);

                                let event = WaitReleaseEvent {
                                    event: Some(wait_release_event::Event::StatusUpdate(
                                        ReleaseStatusUpdate {
                                            destination: status_info.destination.clone(),
                                            status: status_info.status.to_string(),
                                        },
                                    )),
                                };

                                if tx.send(Ok(event)).await.is_err() {
                                    return;
                                }
                            }

                            let log_cursor =
                                *log_cursors.get(&status_info.destination_id).unwrap_or(&-1);

                            match logs_registry
                                .get_logs_after_sequence(
                                    release_intent_id,
                                    status_info.destination_id,
                                    log_cursor,
                                )
                                .await
                            {
                                Ok(log_blocks) => {
                                    for block in log_blocks {
                                        if block.sequence > log_cursor {
                                            log_cursors
                                                .insert(status_info.destination_id, block.sequence);
                                        }

                                        for log_line in block.log_lines {
                                            let event = WaitReleaseEvent {
                                                event: Some(wait_release_event::Event::LogLine(
                                                    ReleaseLogLine {
                                                        destination: status_info.destination.clone(),
                                                        line: log_line.line,
                                                        timestamp: log_line.timestamp.to_string(),
                                                        channel: match log_line.channel {
                                                            LogChannel::Stdout => {
                                                                forest_grpc_interface::LogChannel::Stdout
                                                                    .into()
                                                            }
                                                            LogChannel::Stderr => {
                                                                forest_grpc_interface::LogChannel::Stderr
                                                                    .into()
                                                            }
                                                        },
                                                    },
                                                )),
                                            };

                                            if tx.send(Ok(event)).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        destination = %status_info.destination,
                                        "error polling logs: {e:#}"
                                    );
                                }
                            }

                            if !status_info.status.is_finalized() {
                                all_finalized = false;
                            }
                        }

                        // For pipeline releases, also check stage_states for completion
                        if !last_stage_statuses.is_empty() {
                            let all_stages_terminal = last_stage_statuses.values().all(|s| {
                                matches!(s.as_str(), "SUCCEEDED" | "FAILED" | "CANCELLED")
                            });
                            if all_finalized && all_stages_terminal {
                                break;
                            }
                            // Pipeline not done yet even if current releases are finalized
                            if !all_stages_terminal {
                                continue;
                            }
                        }

                        if all_finalized {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("error polling release status: {e:#}");
                        let _ = tx
                            .send(Err(tonic::Status::internal(format!(
                                "error polling status: {e}"
                            ))))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_releases_by_actor(
        &self,
        request: tonic::Request<GetReleasesByActorRequest>,
    ) -> std::result::Result<tonic::Response<GetReleasesByActorResponse>, tonic::Status> {
        let _actor = request
            .extensions()
            .get::<Actor>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))?;

        let req = request.into_inner();

        let actor_id: uuid::Uuid = req
            .actor_id
            .parse()
            .context("invalid actor_id")
            .to_internal_error()?;

        let valid_types = ["user", "app"];
        if !valid_types.contains(&req.actor_type.as_str()) {
            return Err(tonic::Status::invalid_argument(
                "actor_type must be 'user' or 'app'",
            ));
        }

        let page_size = if req.page_size > 0 {
            req.page_size as i64
        } else {
            20
        };
        let offset = req.page_token.parse::<i64>().unwrap_or(0);

        let results = self
            .state
            .release_registry()
            .get_releases_by_actor(&actor_id, &req.actor_type, page_size, offset)
            .await
            .context("get releases by actor")
            .to_internal_error()?;

        let has_more = results.len() as i64 >= page_size;
        let next_page_token = if has_more {
            (offset + page_size).to_string()
        } else {
            String::new()
        };

        Ok(Response::new(GetReleasesByActorResponse {
            releases: results
                .into_iter()
                .map(|r| ReleaseIntentSummary {
                    release_intent_id: r.release_intent_id.to_string(),
                    artifact_id: r.artifact_id.to_string(),
                    project: Some(r.project.into()),
                    destinations: r
                        .destinations
                        .into_iter()
                        .map(|d| ReleaseDestinationStatus {
                            destination: d.destination,
                            environment: d.environment,
                            status: d.status,
                        })
                        .collect(),
                    created_at: r.created_at.to_rfc3339(),
                })
                .collect(),
            next_page_token,
        }))
    }

    async fn get_organisations(
        &self,
        request: tonic::Request<GetOrganisationsRequest>,
    ) -> std::result::Result<tonic::Response<GetOrganisationsResponse>, tonic::Status> {
        tracing::debug!("get organisations");
        let _req = request.into_inner();

        let organisations = self
            .state
            .release_registry()
            .get_organisations()
            .await
            .context("failed to find organisations")
            .to_internal_error()?;

        Ok(Response::new(GetOrganisationsResponse {
            organisations: organisations.into_iter().map(|n| n.into()).collect(),
        }))
    }

    async fn get_projects(
        &self,
        request: tonic::Request<GetProjectsRequest>,
    ) -> std::result::Result<tonic::Response<GetProjectsResponse>, tonic::Status> {
        let req = request.into_inner();
        tracing::debug!("get projects: {req:?}");

        let projects = match req.query.context("query is required").to_internal_error()? {
            get_projects_request::Query::Organisation(organisation) => self
                .state
                .release_registry()
                .get_projects_by_organisation(&organisation.into())
                .await
                .context("failed to find projects")
                .to_internal_error()?,
        };

        Ok(Response::new(GetProjectsResponse {
            projects: projects.into_iter().map(|n| n.to_string()).collect(),
        }))
    }

    async fn create_project(
        &self,
        request: tonic::Request<CreateProjectRequest>,
    ) -> std::result::Result<tonic::Response<CreateProjectResponse>, tonic::Status> {
        let req = request.into_inner();
        tracing::debug!(
            organisation = %req.organisation,
            project = %req.project,
            "create project"
        );

        self.state
            .release_registry()
            .create_project(&req.organisation, &req.project)
            .await
            .context("create project")
            .to_internal_error()?;

        Ok(Response::new(CreateProjectResponse {
            project: Some(Project {
                organisation: req.organisation,
                project: req.project,
            }),
        }))
    }

    async fn get_destination_states(
        &self,
        request: tonic::Request<GetDestinationStatesRequest>,
    ) -> std::result::Result<tonic::Response<GetDestinationStatesResponse>, tonic::Status> {
        let req = request.into_inner();

        let project_id = if let Some(project) = &req.project {
            let id = self
                .state
                .release_registry()
                .get_project_id(&req.organisation, project)
                .await
                .context("resolve project")
                .to_internal_error()?;
            Some(id)
        } else {
            None
        };

        let event_store = self.state.release_event_store();

        let rows = event_store
            .get_destination_states(&req.organisation, project_id.as_ref())
            .await
            .context("get destination states")
            .to_internal_error()?;

        let destinations = rows
            .into_iter()
            .map(|r| {
                forest_grpc_interface::DestinationState {
                    destination_id: r.destination_id.to_string(),
                    destination_name: r.destination_name,
                    environment: r.environment,
                    release_id: Some(r.release_id.to_string()),
                    artifact_id: Some(r.artifact_id.to_string()),
                    status: Some(r.status),
                    error_message: r.error_message,
                    queued_at: Some(r.queued_at.to_rfc3339()),
                    assigned_at: r.assigned_at.map(|t| t.to_rfc3339()),
                    started_at: r.started_at.map(|t| t.to_rfc3339()),
                    completed_at: r.completed_at.map(|t| t.to_rfc3339()),
                    queue_position: r.queue_position.map(|p| p as i32),
                    release_intent_id: Some(r.release_intent_id.to_string()),
                    stage_id: r.stage_id,
                }
            })
            .collect();

        // Pipeline run data has moved to GetReleaseIntentStates.
        // Keep the field empty for backwards compatibility.
        Ok(Response::new(GetDestinationStatesResponse {
            destinations,
            pipeline_runs: Vec::new(),
        }))
    }

    async fn get_release_intent_states(
        &self,
        request: tonic::Request<GetReleaseIntentStatesRequest>,
    ) -> std::result::Result<tonic::Response<GetReleaseIntentStatesResponse>, tonic::Status> {
        let req = request.into_inner();

        let project_id = if let Some(project) = &req.project {
            let id = self
                .state
                .release_registry()
                .get_project_id(&req.organisation, project)
                .await
                .context("resolve project")
                .to_internal_error()?;
            Some(id)
        } else {
            None
        };

        let event_store = self.state.release_event_store();

        let results = event_store
            .get_release_intent_states(&req.organisation, project_id.as_ref(), req.include_completed)
            .await
            .context("get release intent states")
            .to_internal_error()?;

        let release_intents = results
            .into_iter()
            .map(|(intent, steps)| {
                let stages = intent_to_stage_states(&intent);
                let proto_steps = steps
                    .into_iter()
                    .map(|s| forest_grpc_interface::ReleaseStepState {
                        release_id: s.release_id.to_string(),
                        stage_id: s.stage_id,
                        destination_name: s.destination_name,
                        environment: s.environment,
                        status: s.status,
                        queued_at: Some(s.queued_at.to_rfc3339()),
                        assigned_at: s.assigned_at.map(|t| t.to_rfc3339()),
                        started_at: s.started_at.map(|t| t.to_rfc3339()),
                        completed_at: s.completed_at.map(|t| t.to_rfc3339()),
                        error_message: s.error_message,
                    })
                    .collect();

                forest_grpc_interface::ReleaseIntentState {
                    release_intent_id: intent.release_intent_id.to_string(),
                    artifact_id: intent.artifact_id.to_string(),
                    project: intent.project,
                    created_at: intent.created_at.to_rfc3339(),
                    stages,
                    steps: proto_steps,
                }
            })
            .collect();

        Ok(Response::new(GetReleaseIntentStatesResponse {
            release_intents,
        }))
    }
}

fn stage_status_to_proto(
    status: &crate::services::release_pipeline::StageStatus,
) -> forest_grpc_interface::PipelineRunStageStatus {
    use crate::services::release_pipeline::StageStatus;
    match status {
        StageStatus::Pending => forest_grpc_interface::PipelineRunStageStatus::Pending,
        StageStatus::Active => forest_grpc_interface::PipelineRunStageStatus::Active,
        StageStatus::Succeeded => forest_grpc_interface::PipelineRunStageStatus::Succeeded,
        StageStatus::Failed => forest_grpc_interface::PipelineRunStageStatus::Failed,
        StageStatus::Cancelled => forest_grpc_interface::PipelineRunStageStatus::Cancelled,
    }
}

fn stage_def_to_type_fields(
    config: &crate::services::release_pipeline::StageConfig,
) -> (i32, Option<String>, Option<i64>) {
    use crate::services::release_pipeline::StageConfig;
    match config {
        StageConfig::Deploy { environment } => (
            forest_grpc_interface::PipelineRunStageType::Deploy as i32,
            Some(environment.clone()),
            None,
        ),
        StageConfig::Wait { duration_seconds } => (
            forest_grpc_interface::PipelineRunStageType::Wait as i32,
            None,
            Some(*duration_seconds),
        ),
    }
}

fn intent_to_stage_states(
    intent: &crate::services::release_event_store::ReleaseIntentRow,
) -> Vec<forest_grpc_interface::PipelineStageState> {
    use crate::services::release_pipeline::{PipelineStages, StageStates};

    let Some(ref stages_json) = intent.stages else {
        return Vec::new();
    };

    let stages: PipelineStages = match serde_json::from_value(stages_json.clone()) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let stage_states: StageStates = intent
        .stage_states
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    stages
        .iter()
        .map(|(id, def)| {
            let (stage_type, environment, duration_seconds) = stage_def_to_type_fields(&def.config);
            let state = stage_states.get(id);

            let (status, queued_at, started_at, completed_at, error_message, wait_until, release_ids) =
                if let Some(s) = state {
                    (
                        stage_status_to_proto(&s.status) as i32,
                        s.queued_at.clone(),
                        s.started_at.clone(),
                        s.completed_at.clone(),
                        s.error_message.clone(),
                        s.wait_until.clone(),
                        s.release_ids.clone().unwrap_or_default(),
                    )
                } else {
                    (
                        forest_grpc_interface::PipelineRunStageStatus::Pending as i32,
                        None, None, None, None, None, Vec::new(),
                    )
                };

            forest_grpc_interface::PipelineStageState {
                stage_id: id.clone(),
                depends_on: def.depends_on.clone(),
                stage_type,
                status,
                queued_at,
                started_at,
                completed_at,
                error_message,
                environment,
                duration_seconds,
                wait_until,
                release_ids,
            }
        })
        .collect()
}

fn pipeline_run_to_proto(
    row: crate::services::release_event_store::PipelineRunRow,
) -> anyhow::Result<forest_grpc_interface::PipelineRunState> {
    use crate::services::release_pipeline::{PipelineStages, StageStates};

    let stages: PipelineStages = row
        .stages
        .map(|v| serde_json::from_value(v))
        .transpose()
        .context("parse pipeline stages")?
        .unwrap_or_default();

    let stage_states: StageStates = row
        .stage_states
        .map(|v| serde_json::from_value(v))
        .transpose()
        .context("parse stage states")?
        .unwrap_or_default();

    let proto_stages = stages
        .iter()
        .map(|(id, def)| {
            let (stage_type, environment, duration_seconds) =
                stage_def_to_type_fields(&def.config);
            let state = stage_states.get(id);

            let (status, queued_at, started_at, completed_at, error_message, wait_until, release_ids) =
                if let Some(s) = state {
                    (
                        stage_status_to_proto(&s.status) as i32,
                        s.queued_at.clone(),
                        s.started_at.clone(),
                        s.completed_at.clone(),
                        s.error_message.clone(),
                        s.wait_until.clone(),
                        s.release_ids.clone().unwrap_or_default(),
                    )
                } else {
                    (
                        forest_grpc_interface::PipelineRunStageStatus::Pending as i32,
                        None, None, None, None, None, Vec::new(),
                    )
                };

            forest_grpc_interface::PipelineRunStage {
                stage_id: id.clone(),
                depends_on: def.depends_on.clone(),
                stage_type,
                status,
                environment,
                duration_seconds,
                queued_at,
                started_at,
                completed_at,
                error_message,
                wait_until,
                release_ids,
            }
        })
        .collect();

    Ok(forest_grpc_interface::PipelineRunState {
        release_intent_id: row.release_intent_id.to_string(),
        artifact_id: row.artifact_id.to_string(),
        created_at: row.created_at.to_rfc3339(),
        stages: proto_stages,
    })
}

impl From<grpc::ArtifactContext> for crate::services::release_registry::ArtifactContext {
    fn from(value: grpc::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
            pr: value.pr,
        }
    }
}

impl From<grpc::Source> for crate::services::release_registry::Source {
    fn from(value: grpc::Source) -> Self {
        Self {
            username: value.user,
            email: value.email,
            source_type: value.source_type,
            run_url: value.run_url,
        }
    }
}

impl From<grpc::Ref> for crate::services::release_registry::Reference {
    fn from(value: grpc::Ref) -> Self {
        Self {
            commit_sha: value.commit_sha,
            commit_branch: value.branch,
            commit_message: value.commit_message,
            version: value.version,
            repo_url: value.repo_url,
        }
    }
}

impl From<crate::services::release_registry::Reference> for grpc::Ref {
    fn from(value: crate::services::release_registry::Reference) -> Self {
        Self {
            commit_sha: value.commit_sha,
            branch: value.commit_branch,
            commit_message: value.commit_message,
            version: value.version,
            repo_url: value.repo_url,
        }
    }
}

impl From<ReleaseAnnotation> for Artifact {
    fn from(value: ReleaseAnnotation) -> Self {
        Self {
            id: value.id.into(),
            artifact_id: value.artifact_id.into(),
            metadata: value.metadata,
            source: Some(value.source.into()),
            context: Some(value.context.into()),
            r#ref: Some(value.reference.into()),
            slug: value.slug,
            project: Some(value.project.into()),
            destinations: value.destinations.into_iter().map(|d| d.into()).collect(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

impl From<ReleaseDestination> for ArtifactDestination {
    fn from(value: ReleaseDestination) -> Self {
        Self {
            name: value.name,
            environment: value.environment,
            type_organisation: value.type_organisation,
            type_name: value.type_name,
            type_version: value.type_version as u64,
            status: value.status,
        }
    }
}

impl From<release_registry::Source> for grpc::Source {
    fn from(value: release_registry::Source) -> Self {
        Self {
            user: value.username,
            email: value.email,
            source_type: value.source_type,
            run_url: value.run_url,
        }
    }
}

impl From<release_registry::ArtifactContext> for grpc::ArtifactContext {
    fn from(value: release_registry::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
            pr: value.pr,
        }
    }
}

impl From<release_registry::Project> for grpc::Project {
    fn from(value: release_registry::Project) -> Self {
        Self {
            organisation: value.organisation,
            project: value.project,
        }
    }
}
