//! In-process puppet of the forest-server `RunnerService`. Runs as a tonic
//! server on an ephemeral port; the test driver holds a [`FakeServer`] handle
//! to inject work assignments and inspect the logs/completions the controller
//! pushes back.
//!
//! Only the RPCs the controller actually calls are implemented. Canned
//! responses for `GetReleaseFiles` / `GetSpecFiles` / `GetProjectInfo` /
//! `GetReleaseAnnotation` are keyed per release_token — the test configures
//! them before dispatching.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use forest_grpc_interface::{
    CompleteReleaseRequest, CompleteReleaseResponse, GetProjectInfoRequest,
    GetReleaseAnnotationRequest, GetReleaseFilesRequest, GetSpecFilesRequest, ProjectInfoResponse,
    PushLogRequest, PushLogResponse, RegisterAck, ReleaseAnnotationResponse, ReleaseFile,
    ReleaseOutcome, RunnerMessage, ServerMessage, WorkAssignment, runner_message, server_message,
};
use forest_grpc_interface::runner_service_server::{RunnerService, RunnerServiceServer};
use futures::StreamExt;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::{TcpListenerStream, UnboundedReceiverStream};
use tonic::{Request, Response, Status, Streaming};

#[derive(Clone, Debug)]
pub struct ReleaseFixture {
    pub release_files: Vec<(String, String)>,
    pub spec_files: Vec<(String, String)>,
    pub organisation: String,
    pub project: String,
    pub annotation: ReleaseAnnotationResponse,
}

impl Default for ReleaseFixture {
    fn default() -> Self {
        Self {
            release_files: Vec::new(),
            spec_files: Vec::new(),
            organisation: "test-org".into(),
            project: "test-project".into(),
            annotation: ReleaseAnnotationResponse::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionRecord {
    pub outcome: ReleaseOutcome,
    pub error_message: String,
    pub plan_output: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub channel: String,
    pub line: String,
    pub timestamp: u64,
}

/// Shared state between the tonic service impl and the [`FakeServer`] handle.
#[derive(Default)]
struct SharedState {
    fixtures: HashMap<String, ReleaseFixture>,
    logs: HashMap<String, Vec<LogRecord>>,
    completions: HashMap<String, CompletionRecord>,
    /// Channel to a connected runner. We assume one runner at a time for tests.
    runner_tx: Option<mpsc::UnboundedSender<ServerMessage>>,
    /// Fires when a completion arrives.
    completion_tx: Option<broadcast::Sender<String>>,
}

/// Test-side handle: lets you inject work, assert on recorded data, and shut
/// down the server when done.
pub struct FakeServer {
    pub addr: SocketAddr,
    state: Arc<Mutex<SharedState>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    completion_rx: broadcast::Receiver<String>,
}

impl FakeServer {
    /// Start the server on an ephemeral localhost port. Returns once the
    /// socket is bound so the controller can connect immediately.
    pub async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind fake forest-server")?;
        let addr = listener.local_addr()?;

        let (completion_tx, completion_rx) = broadcast::channel(32);
        let state = Arc::new(Mutex::new(SharedState {
            completion_tx: Some(completion_tx),
            ..Default::default()
        }));

        let service = RunnerServiceImpl {
            state: state.clone(),
        };
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let incoming = TcpListenerStream::new(listener);
        let join = tokio::spawn(async move {
            let result = tonic::transport::Server::builder()
                .add_service(RunnerServiceServer::new(service))
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await;
            if let Err(e) = result {
                tracing::error!(error = %e, "fake forest-server errored");
            }
        });

        Ok(Self {
            addr,
            state,
            shutdown: Some(shutdown_tx),
            join: Some(join),
            completion_rx,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Register the canned responses for a release before dispatching.
    pub fn install_fixture(&self, release_token: &str, fixture: ReleaseFixture) {
        self.state
            .lock()
            .expect("fake server lock poisoned")
            .fixtures
            .insert(release_token.to_string(), fixture);
    }

    /// Push a work assignment to the connected runner. The runner must already
    /// be registered (await [`wait_for_runner`](Self::wait_for_runner) first).
    pub fn dispatch(&self, assignment: WorkAssignment) -> anyhow::Result<()> {
        let tx = self
            .state
            .lock()
            .expect("fake server lock poisoned")
            .runner_tx
            .clone()
            .context("no runner connected — call wait_for_runner first")?;
        tx.send(ServerMessage {
            message: Some(server_message::Message::WorkAssignment(assignment)),
        })
        .map_err(|_| anyhow::anyhow!("runner stream closed"))?;
        Ok(())
    }

    /// Block (with timeout) until a runner registers. Returns once the stream
    /// has been accepted and the `RegisterAck` has been sent.
    pub async fn wait_for_runner(&self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let s = self.state.lock().expect("fake server lock poisoned");
                if s.runner_tx.is_some() {
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("no runner registered within {:?}", timeout);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Block until a completion is reported for `release_token`. Returns the
    /// completion record and the full list of log lines recorded for it.
    pub async fn wait_for_completion(
        &mut self,
        release_token: &str,
        timeout: Duration,
    ) -> anyhow::Result<(CompletionRecord, Vec<LogRecord>)> {
        // Fast-path: completion already arrived.
        {
            let s = self.state.lock().expect("fake server lock poisoned");
            if let Some(c) = s.completions.get(release_token) {
                return Ok((c.clone(), s.logs.get(release_token).cloned().unwrap_or_default()));
            }
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("completion for {release_token} not received within {timeout:?}");
            }
            let recv = tokio::time::timeout(remaining, self.completion_rx.recv()).await;
            match recv {
                Err(_) => bail!("completion for {release_token} not received within {timeout:?}"),
                Ok(Err(_)) => bail!("completion channel closed"),
                Ok(Ok(token)) => {
                    if token != release_token {
                        continue;
                    }
                    let s = self.state.lock().expect("fake server lock poisoned");
                    let completion = s
                        .completions
                        .get(release_token)
                        .cloned()
                        .context("completion reported then vanished")?;
                    let logs = s.logs.get(release_token).cloned().unwrap_or_default();
                    return Ok((completion, logs));
                }
            }
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

// -- tonic service impl -----------------------------------------------------

struct RunnerServiceImpl {
    state: Arc<Mutex<SharedState>>,
}

type StreamResponse<T> =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl RunnerService for RunnerServiceImpl {
    type RegisterRunnerStream = StreamResponse<ServerMessage>;
    type GetReleaseFilesStream = StreamResponse<ReleaseFile>;
    type GetSpecFilesStream = StreamResponse<ReleaseFile>;

    async fn register_runner(
        &self,
        request: Request<Streaming<RunnerMessage>>,
    ) -> Result<Response<Self::RegisterRunnerStream>, Status> {
        let mut inbound = request.into_inner();

        let register = match inbound.next().await {
            Some(Ok(msg)) => match msg.message {
                Some(runner_message::Message::Register(r)) => r,
                _ => return Err(Status::invalid_argument("first message must be Register")),
            },
            Some(Err(e)) => return Err(e),
            None => return Err(Status::cancelled("stream closed before Register")),
        };

        let runner_id = if register.runner_id.is_empty() {
            format!("runner-{}", uuid::Uuid::new_v4())
        } else {
            register.runner_id.clone()
        };
        tracing::info!(runner_id = %runner_id, caps = ?register.capabilities, "puppet: runner registered");

        let (tx, rx) = mpsc::unbounded_channel::<ServerMessage>();
        tx.send(ServerMessage {
            message: Some(server_message::Message::RegisterAck(RegisterAck {
                runner_id: runner_id.clone(),
                accepted: true,
                reason: String::new(),
            })),
        })
        .map_err(|_| Status::internal("failed to queue RegisterAck"))?;

        {
            let mut s = self.state.lock().expect("fake server lock poisoned");
            s.runner_tx = Some(tx);
        }

        let state = self.state.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(m) => match m.message {
                        Some(runner_message::Message::Heartbeat(_)) => {}
                        Some(runner_message::Message::WorkAck(_)) => {}
                        Some(runner_message::Message::Register(_)) => {
                            tracing::warn!("puppet: unexpected re-register");
                        }
                        None => {}
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "puppet: runner stream error");
                        break;
                    }
                }
            }
            let mut s = state.lock().expect("fake server lock poisoned");
            s.runner_tx = None;
        });

        let stream = UnboundedReceiverStream::new(rx).map(Ok);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_release_files(
        &self,
        request: Request<GetReleaseFilesRequest>,
    ) -> Result<Response<Self::GetReleaseFilesStream>, Status> {
        let token = request.into_inner().release_token;
        let files = self.lookup_fixture(&token)?.release_files;
        Ok(Response::new(stream_files(files)))
    }

    async fn get_spec_files(
        &self,
        request: Request<GetSpecFilesRequest>,
    ) -> Result<Response<Self::GetSpecFilesStream>, Status> {
        let token = request.into_inner().release_token;
        let files = self.lookup_fixture(&token)?.spec_files;
        Ok(Response::new(stream_files(files)))
    }

    async fn get_release_annotation(
        &self,
        request: Request<GetReleaseAnnotationRequest>,
    ) -> Result<Response<ReleaseAnnotationResponse>, Status> {
        let token = request.into_inner().release_token;
        let ann = self.lookup_fixture(&token)?.annotation;
        Ok(Response::new(ann))
    }

    async fn get_project_info(
        &self,
        request: Request<GetProjectInfoRequest>,
    ) -> Result<Response<ProjectInfoResponse>, Status> {
        let token = request.into_inner().release_token;
        let f = self.lookup_fixture(&token)?;
        Ok(Response::new(ProjectInfoResponse {
            organisation: f.organisation,
            project: f.project,
        }))
    }

    async fn push_logs(
        &self,
        request: Request<Streaming<PushLogRequest>>,
    ) -> Result<Response<PushLogResponse>, Status> {
        let mut inbound = request.into_inner();
        while let Some(msg) = inbound.next().await {
            match msg {
                Ok(m) => {
                    // Surface each log line as it arrives so `mise run test`
                    // actually shows guest output instead of a silent pass.
                    eprintln!("[{channel}]  {line}", channel = m.channel, line = m.line);

                    let mut s = self.state.lock().expect("fake server lock poisoned");
                    s.logs.entry(m.release_token).or_default().push(LogRecord {
                        channel: m.channel,
                        line: m.line,
                        timestamp: m.timestamp,
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Response::new(PushLogResponse {}))
    }

    async fn complete_release(
        &self,
        request: Request<CompleteReleaseRequest>,
    ) -> Result<Response<CompleteReleaseResponse>, Status> {
        let req = request.into_inner();
        let token = req.release_token.clone();
        let outcome = ReleaseOutcome::try_from(req.outcome).unwrap_or(ReleaseOutcome::Unspecified);
        let record = CompletionRecord {
            outcome,
            error_message: req.error_message,
            plan_output: req.plan_output,
        };
        let completion_tx = {
            let mut s = self.state.lock().expect("fake server lock poisoned");
            s.completions.insert(token.clone(), record);
            s.completion_tx.clone()
        };
        if let Some(tx) = completion_tx {
            let _ = tx.send(token);
        }
        Ok(Response::new(CompleteReleaseResponse {}))
    }
}

impl RunnerServiceImpl {
    fn lookup_fixture(&self, token: &str) -> Result<ReleaseFixture, Status> {
        self.state
            .lock()
            .expect("fake server lock poisoned")
            .fixtures
            .get(token)
            .cloned()
            .ok_or_else(|| Status::not_found(format!("no fixture for token `{token}`")))
    }
}

fn stream_files(files: Vec<(String, String)>) -> StreamResponse<ReleaseFile> {
    let (tx, rx) = mpsc::unbounded_channel::<Result<ReleaseFile, Status>>();
    tokio::spawn(async move {
        for (name, content) in files {
            if tx
                .send(Ok(ReleaseFile {
                    file_name: name,
                    file_content: content,
                }))
                .is_err()
            {
                break;
            }
        }
    });
    Box::pin(UnboundedReceiverStream::new(rx))
}
