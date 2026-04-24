//! Dispatcher: connects to forest-server as a runner, receives WorkAssignments,
//! translates them into RunJob messages, and dispatches to agents.
//! Forwards logs and completion status back to forest-server.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use forest_grpc_interface::{
    CompleteReleaseRequest, DestinationCapability, PushLogRequest, ReleaseMode, ReleaseOutcome,
    WorkAssignment, runner_service_client::RunnerServiceClient,
};
use forest_runner::client::ForestRunnerClient;
use hollow_grpc_interface::RunJob;
use notmad::{Component, ComponentInfo, MadError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent_pool::AgentPool;
use crate::job_tracker::{JobEvent, JobHandle, JobTracker};
use crate::state::{AgentPoolState, JobTrackerState, State};

/// Default resource allocation per job.
pub const DEFAULT_VCPUS_PER_JOB: u32 = 1;
pub const DEFAULT_MEMORY_MIB_PER_JOB: u32 = 1024;
pub const DEFAULT_TIMEOUT_SECONDS: u32 = 1800;

pub struct Dispatcher {
    client: ForestRunnerClient,
    server_addr: String,
    pool: AgentPool,
    tracker: JobTracker,
    runner_id: String,
    capabilities: Vec<DestinationCapability>,
    max_concurrent: i32,
}

pub trait DispatcherState {
    fn dispatcher(
        &self,
        client: ForestRunnerClient,
        runner_id: String,
        capabilities: Vec<DestinationCapability>,
        max_concurrent: i32,
    ) -> Dispatcher;
}

impl DispatcherState for State {
    fn dispatcher(
        &self,
        client: ForestRunnerClient,
        runner_id: String,
        capabilities: Vec<DestinationCapability>,
        max_concurrent: i32,
    ) -> Dispatcher {
        Dispatcher {
            client,
            server_addr: self.server_addr.clone(),
            pool: self.agent_pool(),
            tracker: self.job_tracker(),
            runner_id,
            capabilities,
            max_concurrent,
        }
    }
}

impl Dispatcher {
    /// Connect a reusable gRPC channel to forest-server for background tasks.
    async fn connect_runner_client(
        &self,
    ) -> anyhow::Result<RunnerServiceClient<tonic::transport::Channel>> {
        let channel = tonic::transport::Channel::from_shared(self.server_addr.clone())
            .context("invalid server address")?
            .connect()
            .await
            .context("failed to connect to forest-server")?;
        Ok(RunnerServiceClient::new(channel))
    }

    async fn run_session(&self) -> anyhow::Result<()> {
        tracing::info!("connecting to forest-server as runner...");

        let mut session = self
            .client
            .connect(
                self.runner_id.clone(),
                self.capabilities.clone(),
                self.max_concurrent,
            )
            .await
            .context("failed to connect to forest-server")?;

        // Reusable client for background CompleteRelease calls
        let background_client = self
            .connect_runner_client()
            .await
            .context("failed to create background client")?;

        tracing::info!("registered with forest-server");

        let hb_sender = session.clone_heartbeat_sender();
        let pool = self.pool.clone();
        let hb_cancel = CancellationToken::new();
        let hb_cancel_clone = hb_cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = hb_cancel_clone.cancelled() => break,
                    _ = interval.tick() => {
                        hb_sender.send_heartbeat(pool.active_job_count() as i32);
                    }
                }
            }
        });

        let result = async {
            loop {
                match session.next_work().await {
                    Some(assignment) => {
                        tracing::info!(
                            release_token = %assignment.release_token,
                            release_id = %assignment.release_id,
                            "received work assignment"
                        );
                        self.handle_assignment(&mut session, &background_client, assignment)
                            .await;
                    }
                    None => {
                        tracing::warn!("forest-server stream closed");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            }
        }
        .await;

        hb_cancel.cancel();
        result
    }

    async fn handle_assignment(
        &self,
        session: &mut forest_runner::client::RunnerSession,
        background_client: &RunnerServiceClient<tonic::transport::Channel>,
        assignment: WorkAssignment,
    ) {
        let release_token = assignment.release_token.clone();
        let result = self
            .dispatch_assignment(session, background_client, &assignment)
            .await;

        if let Err(e) = result {
            tracing::error!(
                release_token = %release_token,
                error = %e,
                "failed to dispatch assignment"
            );
            if let Err(report_err) = session
                .complete_release(
                    &release_token,
                    ReleaseOutcome::Failure,
                    Some(&format!("{e:#}")),
                    None,
                )
                .await
            {
                tracing::error!(error = %report_err, "failed to report failure");
            }
        }
    }

    async fn dispatch_assignment(
        &self,
        session: &mut forest_runner::client::RunnerSession,
        background_client: &RunnerServiceClient<tonic::transport::Channel>,
        assignment: &WorkAssignment,
    ) -> anyhow::Result<()> {
        let release_token = &assignment.release_token;
        let destination = assignment
            .destination
            .as_ref()
            .context("missing destination info")?;

        tracing::info!(release_token, "prefetching release data");

        let deployment_files = session
            .get_release_files(release_token)
            .await
            .context("failed to fetch deployment files")?;

        let spec_files = session
            .get_spec_files(release_token)
            .await
            .context("failed to fetch spec files")?;

        let (org, project) = session
            .get_project_info(release_token)
            .await
            .context("failed to fetch project info")?;

        let mode = match ReleaseMode::try_from(assignment.mode) {
            Ok(ReleaseMode::Plan) => "plan",
            Ok(ReleaseMode::Deploy) | Ok(ReleaseMode::Unspecified) | Err(_) => "deploy",
        };

        let dest_type = destination
            .r#type
            .as_ref()
            .context("missing destination type")?;
        let image = format!("{}-v{}", dest_type.name, dest_type.version);
        let command = build_command_for_destination(&dest_type.name, mode, &destination.metadata);

        let mut environment: HashMap<String, String> = destination.metadata.clone();
        environment.insert("FOREST_ORGANISATION".to_string(), org);
        environment.insert("FOREST_PROJECT".to_string(), project);
        environment.insert("FOREST_DESTINATION".to_string(), destination.name.clone());
        environment.insert(
            "FOREST_ENVIRONMENT".to_string(),
            destination.environment.clone(),
        );

        let files: Vec<hollow_grpc_interface::JobFile> = deployment_files
            .iter()
            .chain(spec_files.iter())
            .map(|(path, content)| hollow_grpc_interface::JobFile {
                path: path.to_string_lossy().to_string(),
                content: content.as_bytes().to_vec(),
                mode: 0o644,
            })
            .collect();

        let job_id = format!("job-{}", uuid::Uuid::new_v4());

        let run_job = RunJob {
            job_id: job_id.clone(),
            image,
            command,
            environment,
            files,
            vcpus: DEFAULT_VCPUS_PER_JOB,
            memory_mib: DEFAULT_MEMORY_MIB_PER_JOB,
            disk_mib: 2048,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            egress_enabled: true,
            mode: mode.to_string(),
        };

        let agent_id = self
            .pool
            .dispatch_job(run_job)
            .context("no agent with capacity available")?;

        tracing::info!(job_id, agent_id, "job dispatched to agent");
        metrics::counter!(crate::metrics::names::JOBS_DISPATCHED).increment(1);
        metrics::gauge!(crate::metrics::names::JOBS_ACTIVE).increment(1.0);

        let job_handle =
            self.tracker
                .register_job(job_id.clone(), release_token.to_string(), agent_id);

        let log_sender = session
            .open_log_stream()
            .await
            .context("failed to open log stream")?;

        // Clone the tonic channel (cheap) for the background task
        let mut client = background_client.clone();
        let release_token_owned = release_token.to_string();

        tokio::spawn(async move {
            forward_job_events(job_handle, log_sender, &release_token_owned, &mut client).await;
        });

        Ok(())
    }
}

/// Forward log and completion events from the job tracker to forest-server.
async fn forward_job_events(
    mut handle: JobHandle,
    log_sender: mpsc::UnboundedSender<PushLogRequest>,
    release_token: &str,
    client: &mut RunnerServiceClient<tonic::transport::Channel>,
) {
    while let Some(event) = handle.rx.recv().await {
        match event {
            JobEvent::Log {
                channel,
                line,
                timestamp,
            } => {
                let _ = log_sender.send(PushLogRequest {
                    release_token: release_token.to_string(),
                    channel,
                    line,
                    timestamp,
                });
            }
            JobEvent::Completed {
                exit_code,
                plan_output,
            } => {
                metrics::gauge!(crate::metrics::names::JOBS_ACTIVE).decrement(1.0);
                let (outcome, error) = if exit_code == 0 {
                    metrics::counter!(crate::metrics::names::JOBS_COMPLETED).increment(1);
                    (ReleaseOutcome::Success, None)
                } else {
                    metrics::counter!(crate::metrics::names::JOBS_FAILED).increment(1);
                    (
                        ReleaseOutcome::Failure,
                        Some(format!("process exited with code {exit_code}")),
                    )
                };
                complete_release(
                    client,
                    release_token,
                    outcome,
                    error.as_deref(),
                    plan_output.as_deref(),
                )
                .await;
                return;
            }
            JobEvent::Failed { error_message } => {
                metrics::gauge!(crate::metrics::names::JOBS_ACTIVE).decrement(1.0);
                metrics::counter!(crate::metrics::names::JOBS_FAILED).increment(1);
                complete_release(
                    client,
                    release_token,
                    ReleaseOutcome::Failure,
                    Some(&error_message),
                    None,
                )
                .await;
                return;
            }
        }
    }

    // Channel closed without completion — agent disconnected
    tracing::warn!(release_token, "job event channel closed without completion");
    metrics::gauge!(crate::metrics::names::JOBS_ACTIVE).decrement(1.0);
    metrics::counter!(crate::metrics::names::JOBS_FAILED).increment(1);
    complete_release(
        client,
        release_token,
        ReleaseOutcome::Failure,
        Some("agent disconnected before job completed"),
        None,
    )
    .await;
}

async fn complete_release(
    client: &mut RunnerServiceClient<tonic::transport::Channel>,
    release_token: &str,
    outcome: ReleaseOutcome,
    error_message: Option<&str>,
    plan_output: Option<&str>,
) {
    if let Err(e) = client
        .complete_release(CompleteReleaseRequest {
            release_token: release_token.to_string(),
            outcome: outcome.into(),
            error_message: error_message.unwrap_or_default().to_string(),
            plan_output: plan_output.map(|s| s.to_string()),
        })
        .await
    {
        tracing::error!(error = %e, "CompleteRelease RPC failed");
    }
}

/// Build the command to run inside the VM based on destination type and mode.
///
/// Adding a new destination? Add an arm. Destinations are matched by the
/// `DestinationCapability.name` the controller is registered with.
fn build_command_for_destination(
    dest_name: &str,
    mode: &str,
    metadata: &HashMap<String, String>,
) -> Vec<String> {
    match dest_name {
        "terraform" => {
            let action = if mode == "plan" {
                "plan -no-color"
            } else {
                "apply -no-color -auto-approve"
            };
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("terraform init -no-color && terraform {action}"),
            ]
        }
        // OpenTofu is the open-source Terraform fork. Same CLI surface, so
        // the command shape is identical; the image is separate because it
        // ships the `tofu` binary + baked providers instead of `terraform`.
        "opentofu" => {
            let action = if mode == "plan" {
                "plan -no-color"
            } else {
                "apply -no-color -auto-approve"
            };
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("tofu init -no-color -input=false && tofu {action} -input=false"),
            ]
        }
        // Test-only: run an arbitrary shell command supplied via destination
        // metadata. Used by hollow-acceptance orchestrator tests to drive the
        // full controller→agent→VM path with a trivial payload.
        "echo" => {
            let script = metadata
                .get("command")
                .cloned()
                .unwrap_or_else(|| "echo hello".to_string());
            vec!["sh".to_string(), "-c".to_string(), script]
        }
        other => {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("echo 'unsupported destination: {other}'; exit 1"),
            ]
        }
    }
}

impl Component for Dispatcher {
    fn info(&self) -> ComponentInfo {
        "hollow/dispatcher".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("dispatcher shutting down");
                    break;
                }
                result = self.run_session() => {
                    match result {
                        Ok(()) => tracing::info!("dispatcher session ended"),
                        Err(e) => tracing::error!(error = %e, "dispatcher session error"),
                    }
                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }

        Ok(())
    }
}
