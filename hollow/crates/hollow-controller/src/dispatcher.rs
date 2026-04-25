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

        // If this destination has a server-issued artifact store (e.g.
        // terraform.tfstate over HTTP), translate the generic URL+auth
        // shape into whatever env vars the destination's tooling expects.
        // Without this, terraform runs from empty state every release and
        // resources end up "created" each run.
        if let Some(store) = assignment.artifact_store.as_ref() {
            tracing::info!(
                store_id = %store.id,
                store_url = %store.url,
                dest_type = %dest_type.name,
                "applying release artifact_store env vars"
            );
            apply_artifact_store_env(&mut environment, &dest_type.name, store);
        }

        // Forest artifacts pack files for every destination of every type
        // under one tarball, with paths shaped like
        //   `<env>/<dest-name-or-regex>/<org>/<type>@<version>/<file>`
        // We're running ONE destination here, so trim the artifact to just
        // its files and strip the prefix so e.g. `main.tf` lands at /work.
        // Spec files (forest.cue etc.) live outside that hierarchy and ride
        // along untouched.
        let dest_files = filter_files_for_destination(
            &deployment_files,
            &destination.environment,
            &destination.name,
            &dest_type.organisation,
            &dest_type.name,
            dest_type.version,
        );
        if dest_files.is_empty() {
            tracing::warn!(
                env = %destination.environment,
                dest = %destination.name,
                ty = format_args!("{}/{}@{}", dest_type.organisation, dest_type.name, dest_type.version),
                total_files = deployment_files.len(),
                "no deployment files matched destination — job will likely fail with empty config"
            );
        }
        let files: Vec<hollow_grpc_interface::JobFile> = dest_files
            .into_iter()
            .chain(spec_files.iter().map(|(path, content)| {
                (path.to_string_lossy().to_string(), content.clone())
            }))
            .map(|(path, content)| hollow_grpc_interface::JobFile {
                path,
                content: content.as_bytes().to_vec(),
                mode: 0o644,
            })
            .collect();

        let job_id = format!("job-{}", uuid::Uuid::new_v4());

        // Per-destination egress allowlist: sourced from destination metadata
        // under the `allowed_egress_cidrs` key (comma-separated CIDRs). When
        // unset, the VM follows the default "public internet only" posture.
        // When set, the VM is restricted to *only* those CIDRs — useful for
        // terraform destinations that should only reach the cloud API or
        // for closed-network deploys.
        let allowed_egress_cidrs = parse_egress_allowlist(&destination.metadata);

        let secrets = build_secrets_for_destination(&dest_type.name, &destination.metadata)
            .context("failed to assemble destination secrets")?;

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
            allowed_egress_cidrs,
            secrets,
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

/// Parse the `allowed_egress_cidrs` destination metadata key into a
/// validated CIDR list. Empty / missing → empty Vec → "no allowlist", which
/// the agent reads as "default egress posture".
///
/// Format: comma-separated CIDR strings, whitespace ignored, e.g.
/// `"1.1.1.1/32, 8.8.8.8/32"` or `"54.239.0.0/16,52.119.128.0/17"`.
fn parse_egress_allowlist(metadata: &HashMap<String, String>) -> Vec<String> {
    let Some(raw) = metadata.get("allowed_egress_cidrs") else {
        return Vec::new();
    };
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Translate the generic `ReleaseArtifactStore` (URL + basic auth) into
/// the env vars a particular destination's tooling expects. Adding a new
/// destination that uses `artifact_store`? Add an arm here.
fn apply_artifact_store_env(
    env: &mut HashMap<String, String>,
    dest_name: &str,
    store: &forest_grpc_interface::ReleaseArtifactStore,
) {
    if dest_name == "terraform" {
        // Terraform's HTTP backend uses {base, base/lock, base/unlock} with
        // a fixed lock/unlock method. The base URL comes from the server.
        env.insert("TF_HTTP_ADDRESS".to_string(), store.url.clone());
        env.insert(
            "TF_HTTP_LOCK_ADDRESS".to_string(),
            format!("{}/lock", store.url),
        );
        env.insert(
            "TF_HTTP_UNLOCK_ADDRESS".to_string(),
            format!("{}/unlock", store.url),
        );
        env.insert("TF_HTTP_USERNAME".to_string(), store.username.clone());
        env.insert("TF_HTTP_PASSWORD".to_string(), store.password.clone());
        env.insert("TF_HTTP_LOCK_METHOD".to_string(), "POST".to_string());
        env.insert("TF_HTTP_UNLOCK_METHOD".to_string(), "POST".to_string());
    }
    // Other destinations that grow into using artifact_store get their own
    // arm here. Default: no env injection.
}

/// Assemble per-destination secrets from metadata. Today only `fluxv1`
/// uses this — it pulls a path out of `metadata.git_ssh_key_path` and
/// ships the file's bytes as a Secret. The path is read from the
/// controller's own filesystem (the controller is the trust boundary
/// for credential material; the agent and guest never see paths, just
/// bytes via the secrets channel).
///
/// Adding a new destination that needs file-backed secrets? Add an arm.
fn build_secrets_for_destination(
    dest_name: &str,
    metadata: &HashMap<String, String>,
) -> anyhow::Result<Vec<hollow_grpc_interface::Secret>> {
    let mut secrets = Vec::new();
    if dest_name == "fluxv1"
        && let Some(path) = metadata.get("git_ssh_key_path").map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        let bytes = std::fs::read(path).with_context(|| {
            format!("reading git SSH key for fluxv1 destination from {path}")
        })?;
        tracing::info!(
            target_path = "/root/.ssh/id_forest",
            source = %path,
            bytes = bytes.len(),
            "shipping git SSH key as fluxv1 Secret"
        );
        secrets.push(hollow_grpc_interface::Secret {
            name: "git_ssh_key".to_string(),
            target_path: "/root/.ssh/id_forest".to_string(),
            mode: 0o600,
            content: bytes,
        });
    }
    Ok(secrets)
}

/// Pick the subset of an artifact's deployment files that belongs to a
/// specific destination, and strip the `<env>/<dest>/<org>/<type>@<ver>/`
/// prefix so files land at the working directory root inside the VM.
///
/// The artifact path layout is shared with the legacy in-process runner
/// (see `crates/forest-server/src/destinations/terraformv1.rs`): each
/// artifact carries files for every (env, destination-name-or-regex,
/// destination-type) tuple the project deploys to. We're running one
/// destination here, so the others are dead weight (and worse, can break
/// tools like `tofu` that scan the working directory recursively).
///
/// The destination-name segment is treated as a regex against `dest_name`,
/// matching the legacy runner's behaviour. Patterns like
/// `infrastructure-dev.*` thus match both `infrastructure-dev/1` and
/// `infrastructure-dev/2`.
fn filter_files_for_destination(
    deployment_files: &[(std::path::PathBuf, String)],
    env: &str,
    dest_name: &str,
    type_org: &str,
    type_name: &str,
    type_version: u64,
) -> Vec<(String, String)> {
    let type_segment = format!("{type_name}@{type_version}");
    let mut out = Vec::with_capacity(deployment_files.len());
    for (path, content) in deployment_files {
        let s = path.to_string_lossy();
        let parts: Vec<&str> = s.split('/').collect();
        // Need at least <env>/<dest>/<org>/<type@version>/<file>
        if parts.len() < 5 {
            continue;
        }
        if parts[0] != env {
            continue;
        }
        let dest_pattern = parts[1];
        let dest_match = match regex::Regex::new(&format!("^{dest_pattern}$")) {
            Ok(re) => re.is_match(dest_name),
            // Pattern wasn't a valid regex — fall back to literal equality.
            Err(_) => dest_pattern == dest_name,
        };
        if !dest_match {
            continue;
        }
        if parts[2] != type_org || parts[3] != type_segment {
            continue;
        }
        let rel = parts[4..].join("/");
        out.push((rel, content.clone()));
    }
    out
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
        // The terraform-v1 image actually ships OpenTofu under a `terraform`
        // symlink, so this command surface is BSL-free even though the
        // destination type is named after Terraform (matching forest-server's
        // existing destination registry).
        "terraform" => {
            let action = if mode == "plan" {
                "plan -no-color"
            } else {
                "apply -no-color -auto-approve"
            };
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("terraform init -no-color -input=false && terraform {action} -input=false"),
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
        // forest/fluxv1/1: clone target git repo, write manifests under
        // releases/<env>/<dest>/<cluster>/<ns>/<project>/, commit, push.
        // The full workflow lives in /usr/local/bin/forest-flux-deploy
        // baked into the image; configuration arrives via env vars
        // (sourced from destination.metadata) and an optional SSH key
        // shipped as a Secret. For tests/diagnostics, supplying
        // metadata.command overrides the default deploy invocation.
        "fluxv1" => match metadata.get("command") {
            Some(cmd) => vec!["sh".to_string(), "-c".to_string(), cmd.clone()],
            None => vec!["/usr/local/bin/forest-flux-deploy".to_string()],
        },
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
