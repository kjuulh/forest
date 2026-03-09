use std::pin::Pin;

use forest_grpc_interface::{runner_service_server::RunnerService, *};
use forest_models::ReleaseStatus;
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Response;
use uuid::Uuid;

use crate::{
    runner_manager::{DestinationCapability, RunnerManager},
    services::{
        artifact_staging_registry::ArtifactStagingRegistryState,
        destination_registry::DestinationRegistryState,
        notification_registry::NotificationRegistryState,
        release_event_store::{
            EventPayload, ReleaseEventStoreState, ReleaseEventType,
        },
        release_finalizer,
        release_logs_registry::{
            LogChannel, LogLine, ReleaseLogsRegistryState,
        },
        release_registry::ReleaseRegistryState,
        release_token_registry::ReleaseTokenRegistryState,
    },
    state::State,
};

pub struct RunnerServer {
    pub state: State,
    pub runner_manager: RunnerManager,
}

#[async_trait::async_trait]
impl RunnerService for RunnerServer {
    type RegisterRunnerStream =
        Pin<Box<dyn Stream<Item = Result<ServerMessage, tonic::Status>> + Send>>;

    async fn register_runner(
        &self,
        request: tonic::Request<tonic::Streaming<RunnerMessage>>,
    ) -> Result<Response<Self::RegisterRunnerStream>, tonic::Status> {
        let mut inbound = request.into_inner();

        // Wait for the first message to be RunnerRegister
        let first_msg = inbound
            .message()
            .await
            .map_err(|e| tonic::Status::internal(format!("stream error: {e}")))?
            .ok_or_else(|| tonic::Status::invalid_argument("stream closed before registration"))?;

        let register = match first_msg.message {
            Some(runner_message::Message::Register(reg)) => reg,
            _ => {
                return Err(tonic::Status::invalid_argument(
                    "first message must be RunnerRegister",
                ))
            }
        };

        let runner_id = if register.runner_id.is_empty() {
            Uuid::now_v7().to_string()
        } else {
            register.runner_id.clone()
        };

        let capabilities: Vec<DestinationCapability> = register
            .capabilities
            .into_iter()
            .map(|c| DestinationCapability {
                organisation: c.organisation,
                name: c.name,
                version: c.version as usize,
            })
            .collect();

        // Channel for the scheduler to send work assignments to this runner
        let (work_tx, mut work_rx) = mpsc::channel::<WorkAssignment>(16);

        self.runner_manager
            .register_runner(
                runner_id.clone(),
                capabilities,
                register.max_concurrent,
                work_tx,
            )
            .await;

        // Outbound channel to the runner
        let (out_tx, out_rx) = mpsc::channel(16);

        // Send RegisterAck
        let ack = ServerMessage {
            message: Some(server_message::Message::RegisterAck(RegisterAck {
                runner_id: runner_id.clone(),
                accepted: true,
                reason: String::new(),
            })),
        };
        let _ = out_tx.send(Ok(ack)).await;

        // Spawn the bidirectional stream handler
        let runner_manager = self.runner_manager.clone();
        let runner_id_clone = runner_id.clone();
        let state_clone = self.state.clone();

        tokio::spawn(async move {
            let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));

            loop {
                tokio::select! {
                    // Work assignment from scheduler → forward to runner
                    work = work_rx.recv() => {
                        match work {
                            Some(assignment) => {
                                let msg = ServerMessage {
                                    message: Some(server_message::Message::WorkAssignment(assignment)),
                                };
                                if out_tx.send(Ok(msg)).await.is_err() {
                                    // Runner disconnected
                                    break;
                                }
                            }
                            None => {
                                // Work channel closed (shouldn't happen normally)
                                break;
                            }
                        }
                    }

                    // Inbound messages from runner (heartbeat, work ack)
                    msg = inbound.message() => {
                        match msg {
                            Ok(Some(runner_msg)) => {
                                match runner_msg.message {
                                    Some(runner_message::Message::Heartbeat(hb)) => {
                                        runner_manager.update_heartbeat(
                                            &runner_id_clone,
                                            hb.active_releases,
                                        ).await;
                                        // Update heartbeat timestamp for all active releases on this runner
                                        if let Err(e) = state_clone.release_event_store()
                                            .heartbeat_runner_releases(&runner_id_clone)
                                            .await
                                        {
                                            tracing::warn!(
                                                runner_id = %runner_id_clone,
                                                "failed to update release heartbeats: {e}"
                                            );
                                        }
                                    }
                                    Some(runner_message::Message::WorkAck(ack)) => {
                                        // Transition ASSIGNED -> RUNNING when runner acks
                                        if ack.accepted {
                                            let event_store = state_clone.release_event_store();
                                            let token_registry = state_clone.release_token_registry();
                                            if let Ok(Some(scope)) = token_registry
                                                .validate_token(&ack.release_token)
                                                .await
                                                && let Err(e) = event_store
                                                    .emit_event(
                                                        scope.release_id,
                                                        ReleaseEventType::Started,
                                                        EventPayload::default(),
                                                        None,
                                                    )
                                                    .await
                                                {
                                                    tracing::warn!(
                                                        release_id = %scope.release_id,
                                                        "failed to transition to RUNNING: {e}"
                                                    );
                                                }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Ok(None) | Err(_) => {
                                // Stream closed or error
                                break;
                            }
                        }
                    }

                    // Periodic server heartbeat (keepalive)
                    _ = heartbeat_interval.tick() => {
                        // No-op for now; the heartbeat from the runner side
                        // keeps the connection alive
                    }
                }
            }

            // Unregister runner on disconnect
            runner_manager.unregister_runner(&runner_id_clone).await;
            tracing::info!(runner_id = %runner_id_clone, "runner stream closed");

            // Recovery: fail any in-flight releases for this runner
            let token_registry = state_clone.release_token_registry();
            let event_store = state_clone.release_event_store();

            match token_registry.revoke_runner_tokens(&runner_id_clone).await {
                Ok(revoked) => {
                    for scope in revoked {
                        if let Err(e) = event_store
                            .emit_event(
                                scope.release_id,
                                ReleaseEventType::Failed,
                                EventPayload {
                                    error_message: Some(format!(
                                        "runner {} disconnected",
                                        runner_id_clone
                                    )),
                                    ..Default::default()
                                },
                                None,
                            )
                            .await
                        {
                            tracing::warn!(
                                release_id = %scope.release_id,
                                "failed to fail orphaned release: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        runner_id = %runner_id_clone,
                        "failed to revoke runner tokens: {e}"
                    );
                }
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(out_rx)) as Self::RegisterRunnerStream
        ))
    }

    type GetReleaseFilesStream =
        Pin<Box<dyn Stream<Item = Result<ReleaseFile, tonic::Status>> + Send>>;

    async fn get_release_files(
        &self,
        request: tonic::Request<GetReleaseFilesRequest>,
    ) -> Result<Response<Self::GetReleaseFilesStream>, tonic::Status> {
        let req = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let scope = token_registry
            .validate_token(&req.release_token)
            .await
            .map_err(|e| tonic::Status::internal(format!("token validation error: {e}")))?
            .ok_or_else(|| {
                tonic::Status::unauthenticated("invalid, expired, or revoked release token")
            })?;

        let artifact_registry = self.state.artifact_staging_registry();
        let files = artifact_registry
            .get_files_for_release(&scope.artifact_id, &scope.environment)
            .await
            .map_err(|e| tonic::Status::internal(format!("failed to get release files: {e}")))?;

        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            for (path, content) in files {
                let file = ReleaseFile {
                    file_name: path.to_string_lossy().to_string(),
                    file_content: content,
                };
                if tx.send(Ok(file)).await.is_err() {
                    break; // Client disconnected
                }
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as Self::GetReleaseFilesStream
        ))
    }

    type GetSpecFilesStream =
        Pin<Box<dyn Stream<Item = Result<ReleaseFile, tonic::Status>> + Send>>;

    async fn get_spec_files(
        &self,
        request: tonic::Request<GetSpecFilesRequest>,
    ) -> Result<Response<Self::GetSpecFilesStream>, tonic::Status> {
        let req = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let scope = token_registry
            .validate_token(&req.release_token)
            .await
            .map_err(|e| tonic::Status::internal(format!("token validation error: {e}")))?
            .ok_or_else(|| {
                tonic::Status::unauthenticated("invalid, expired, or revoked release token")
            })?;

        let artifact_registry = self.state.artifact_staging_registry();
        let files = artifact_registry
            .get_spec_files(&scope.artifact_id)
            .await
            .map_err(|e| tonic::Status::internal(format!("failed to get spec files: {e}")))?;

        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            for (path, content) in files {
                let file = ReleaseFile {
                    file_name: path.to_string_lossy().to_string(),
                    file_content: content,
                };
                if tx.send(Ok(file)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as Self::GetSpecFilesStream
        ))
    }

    async fn get_release_annotation(
        &self,
        request: tonic::Request<GetReleaseAnnotationRequest>,
    ) -> Result<Response<ReleaseAnnotationResponse>, tonic::Status> {
        let req = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let scope = token_registry
            .validate_token(&req.release_token)
            .await
            .map_err(|e| tonic::Status::internal(format!("token validation error: {e}")))?
            .ok_or_else(|| {
                tonic::Status::unauthenticated("invalid, expired, or revoked release token")
            })?;

        let rec = sqlx::query!(
            "SELECT slug, source, context, ref, created FROM annotations WHERE artifact_id = $1",
            scope.artifact_id
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| tonic::Status::internal(format!("failed to get annotation: {e}")))?;

        // Extract fields from JSONB columns
        fn json_str(val: &serde_json::Value, key: &str) -> String {
            val.get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        }

        let source = &rec.source;
        let context = &rec.context;
        let reference = &rec.r#ref;

        Ok(Response::new(ReleaseAnnotationResponse {
            slug: rec.slug,
            source_username: json_str(source, "username"),
            source_email: json_str(source, "email"),
            context_title: json_str(context, "title"),
            context_description: json_str(context, "description"),
            context_web: json_str(context, "web"),
            reference_version: json_str(reference, "version"),
            reference_commit_sha: json_str(reference, "commit_sha"),
            reference_commit_branch: json_str(reference, "commit_branch"),
            reference_commit_message: json_str(reference, "commit_message"),
            created_at: rec.created.to_rfc3339(),
        }))
    }

    async fn get_project_info(
        &self,
        request: tonic::Request<GetProjectInfoRequest>,
    ) -> Result<Response<ProjectInfoResponse>, tonic::Status> {
        let req = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let scope = token_registry
            .validate_token(&req.release_token)
            .await
            .map_err(|e| tonic::Status::internal(format!("token validation error: {e}")))?
            .ok_or_else(|| {
                tonic::Status::unauthenticated("invalid, expired, or revoked release token")
            })?;

        let rec = sqlx::query!(
            "SELECT organisation, project FROM projects WHERE id = $1",
            scope.project_id
        )
        .fetch_one(&self.state.db)
        .await
        .map_err(|e| tonic::Status::internal(format!("failed to get project info: {e}")))?;

        Ok(Response::new(ProjectInfoResponse {
            organisation: rec.organisation,
            project: rec.project,
        }))
    }

    async fn push_logs(
        &self,
        request: tonic::Request<tonic::Streaming<PushLogRequest>>,
    ) -> Result<Response<PushLogResponse>, tonic::Status> {
        let mut stream = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let logs_registry = self.state.release_logs_registry();

        let mut validated_scope = None;
        let attempt = Uuid::now_v7();
        let mut buffer: Vec<LogLine> = Vec::new();
        let mut sequence: i64 = 0;

        while let Some(msg) = stream
            .message()
            .await
            .map_err(|e| tonic::Status::internal(format!("stream error: {e}")))?
        {
            // Validate token on first message
            if validated_scope.is_none() {
                let scope = token_registry
                    .validate_token(&msg.release_token)
                    .await
                    .map_err(|e| {
                        tonic::Status::internal(format!("token validation error: {e}"))
                    })?
                    .ok_or_else(|| {
                        tonic::Status::unauthenticated(
                            "invalid, expired, or revoked release token",
                        )
                    })?;
                validated_scope = Some(scope);
            }

            let scope = validated_scope.as_ref().unwrap();

            let channel = match msg.channel.as_str() {
                "stderr" => LogChannel::Stderr,
                _ => LogChannel::Stdout,
            };

            buffer.push(LogLine {
                channel,
                line: msg.line,
                timestamp: msg.timestamp as u128,
            });

            // Flush when buffer is large enough
            if buffer.len() >= 100 {
                logs_registry
                    .insert_log_block(
                        attempt,
                        scope.release_intent_id,
                        scope.destination_id,
                        &buffer,
                        sequence,
                    )
                    .await
                    .map_err(|e| {
                        tonic::Status::internal(format!("failed to insert log block: {e}"))
                    })?;
                buffer.clear();
                sequence += 1;
            }
        }

        // Flush remaining
        if !buffer.is_empty()
            && let Some(scope) = &validated_scope {
                let _ = logs_registry
                    .insert_log_block(
                        attempt,
                        scope.release_intent_id,
                        scope.destination_id,
                        &buffer,
                        sequence,
                    )
                    .await;
            }

        Ok(Response::new(PushLogResponse {}))
    }

    async fn complete_release(
        &self,
        request: tonic::Request<CompleteReleaseRequest>,
    ) -> Result<Response<CompleteReleaseResponse>, tonic::Status> {
        let req = request.into_inner();

        let token_registry = self.state.release_token_registry();
        let scope = token_registry
            .validate_token(&req.release_token)
            .await
            .map_err(|e| tonic::Status::internal(format!("token validation error: {e}")))?
            .ok_or_else(|| {
                tonic::Status::unauthenticated("invalid, expired, or revoked release token")
            })?;

        let status = match req.outcome() {
            ReleaseOutcome::Success => ReleaseStatus::Succeeded,
            ReleaseOutcome::Failure => ReleaseStatus::Failed,
            ReleaseOutcome::Unspecified => {
                return Err(tonic::Status::invalid_argument(
                    "outcome must be SUCCESS or FAILURE",
                ))
            }
        };

        let error_message = if status.is_failure() && !req.error_message.is_empty() {
            Some(req.error_message.as_str())
        } else {
            None
        };

        // Finalize: emit event + create notification
        release_finalizer::finalize_release(
            &self.state.release_event_store(),
            &self.state.release_registry(),
            &self.state.notification_registry(),
            &self.state.destination_registry(),
            &scope.release_id,
            &scope.release_intent_id,
            &scope.artifact_id,
            &scope.project_id,
            &scope.destination_id,
            status,
            error_message,
        )
        .await
        .map_err(|e| tonic::Status::internal(format!("failed to finalize release: {e}")))?;

        // Revoke the token
        if let Err(e) = token_registry.revoke_token(&req.release_token).await {
            tracing::warn!("failed to revoke release token: {e:#}");
        }

        // Decrement runner's active releases
        self.runner_manager
            .release_completed(&scope.runner_id)
            .await;

        tracing::info!(
            release_id = %scope.release_id,
            runner_id = %scope.runner_id,
            %status,
            "runner completed release"
        );

        Ok(Response::new(CompleteReleaseResponse {}))
    }
}
