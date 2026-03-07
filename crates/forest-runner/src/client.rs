use std::path::PathBuf;

use anyhow::Context;
use forest_grpc_interface::{
    DestinationCapability, GetProjectInfoRequest, GetReleaseAnnotationRequest,
    GetReleaseFilesRequest, GetSpecFilesRequest, PushLogRequest, ReleaseAnnotationResponse,
    RunnerHeartbeat, RunnerMessage, RunnerRegister, WorkAssignment, runner_message,
    runner_service_client::RunnerServiceClient,
};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Client that connects to a forest-server RunnerService.
pub struct ForestRunnerClient {
    addr: String,
}

impl ForestRunnerClient {
    pub fn new(addr: String) -> Self {
        Self { addr }
    }

    /// Connect to the server, register as a runner, and return a session
    /// that receives work assignments and can make authenticated calls.
    pub async fn connect(
        &self,
        runner_id: String,
        capabilities: Vec<DestinationCapability>,
        max_concurrent: i32,
    ) -> anyhow::Result<RunnerSession> {
        let channel = tonic::transport::Channel::from_shared(self.addr.clone())
            .context("invalid server address")?
            .connect()
            .await
            .context("failed to connect to forest-server")?;

        let mut client = RunnerServiceClient::new(channel.clone());

        // Create the outbound stream for RegisterRunner
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<RunnerMessage>();

        // Send registration message
        outbound_tx.send(RunnerMessage {
            message: Some(runner_message::Message::Register(RunnerRegister {
                runner_id: runner_id.clone(),
                capabilities,
                max_concurrent,
            })),
        })?;

        let outbound_stream = UnboundedReceiverStream::new(outbound_rx);

        let response = client
            .register_runner(outbound_stream)
            .await
            .context("failed to register runner")?;

        let inbound = response.into_inner();

        Ok(RunnerSession {
            client: RunnerServiceClient::new(channel),
            outbound_tx,
            inbound,
        })
    }
}

/// An active runner session with the forest-server.
pub struct RunnerSession {
    client: RunnerServiceClient<tonic::transport::Channel>,
    outbound_tx: mpsc::UnboundedSender<RunnerMessage>,
    inbound: tonic::Streaming<forest_grpc_interface::ServerMessage>,
}

impl RunnerSession {
    /// Wait for the next work assignment from the server.
    /// Returns None if the stream is closed.
    pub async fn next_work(&mut self) -> Option<WorkAssignment> {
        loop {
            match self.inbound.next().await {
                Some(Ok(msg)) => {
                    if let Some(forest_grpc_interface::server_message::Message::WorkAssignment(
                        assignment,
                    )) = msg.message
                    {
                        return Some(assignment);
                    }
                    // RegisterAck or other messages — skip
                }
                Some(Err(e)) => {
                    tracing::error!("stream error: {e}");
                    return None;
                }
                None => return None,
            }
        }
    }

    /// Send a heartbeat to the server.
    pub fn send_heartbeat(&self, active_releases: i32) {
        let _ = self.outbound_tx.send(RunnerMessage {
            message: Some(runner_message::Message::Heartbeat(RunnerHeartbeat {
                active_releases,
            })),
        });
    }

    /// Fetch artifact files for a release.
    pub async fn get_release_files(
        &mut self,
        release_token: &str,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let response = self
            .client
            .get_release_files(GetReleaseFilesRequest {
                release_token: release_token.to_string(),
            })
            .await
            .context("failed to get release files")?;

        let mut stream = response.into_inner();
        let mut files = Vec::new();

        while let Some(file) = stream.next().await {
            let file = file.context("error streaming release file")?;
            files.push((PathBuf::from(file.file_name), file.file_content));
        }

        Ok(files)
    }

    /// Fetch spec (original) files for a release.
    pub async fn get_spec_files(
        &mut self,
        release_token: &str,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let response = self
            .client
            .get_spec_files(GetSpecFilesRequest {
                release_token: release_token.to_string(),
            })
            .await
            .context("failed to get spec files")?;

        let mut stream = response.into_inner();
        let mut files = Vec::new();

        while let Some(file) = stream.next().await {
            let file = file.context("error streaming spec file")?;
            files.push((PathBuf::from(file.file_name), file.file_content));
        }

        Ok(files)
    }

    /// Fetch the release annotation (metadata context).
    pub async fn get_release_annotation(
        &mut self,
        release_token: &str,
    ) -> anyhow::Result<ReleaseAnnotationResponse> {
        let response = self
            .client
            .get_release_annotation(GetReleaseAnnotationRequest {
                release_token: release_token.to_string(),
            })
            .await
            .context("failed to get release annotation")?;

        Ok(response.into_inner())
    }

    /// Fetch project info (organisation + project name).
    pub async fn get_project_info(
        &mut self,
        release_token: &str,
    ) -> anyhow::Result<(String, String)> {
        let response = self
            .client
            .get_project_info(GetProjectInfoRequest {
                release_token: release_token.to_string(),
            })
            .await
            .context("failed to get project info")?;

        let info = response.into_inner();
        Ok((info.organisation, info.project))
    }

    /// Open a log stream for pushing lines to the server.
    /// Returns a sender for log messages and a handle that completes when the stream closes.
    pub async fn open_log_stream(
        &mut self,
    ) -> anyhow::Result<mpsc::UnboundedSender<PushLogRequest>> {
        let (tx, rx) = mpsc::unbounded_channel::<PushLogRequest>();
        let stream = UnboundedReceiverStream::new(rx);

        let mut client = self.client.clone();
        tokio::spawn(async move {
            if let Err(e) = client.push_logs(stream).await {
                tracing::warn!("push_logs stream error: {e}");
            }
        });

        Ok(tx)
    }

    /// Report release completion.
    pub async fn complete_release(
        &mut self,
        release_token: &str,
        outcome: forest_grpc_interface::ReleaseOutcome,
        error_message: Option<&str>,
    ) -> anyhow::Result<()> {
        self.client
            .complete_release(forest_grpc_interface::CompleteReleaseRequest {
                release_token: release_token.to_string(),
                outcome: outcome.into(),
                error_message: error_message.unwrap_or_default().to_string(),
            })
            .await
            .context("failed to complete release")?;

        Ok(())
    }
}
