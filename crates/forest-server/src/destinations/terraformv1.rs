use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    process::Stdio,
    sync::{Arc, OnceLock},
};

use anyhow::Context;
use axum::{
    Json,
    extract::{Path, Query, State as AState},
    routing::post,
};
use axum::{response::IntoResponse, routing::get};
use forest_models::Destination;

use http::StatusCode;
use notmad::{Component, MadError};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::Span;

use crate::{
    State,
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem},
    temp_dir::TempDirectories,
};

#[derive(Clone)]
pub struct TerraformStateStore {
    // TODO: move to some kind of database
    states: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    users: Arc<Mutex<BTreeMap<String, String>>>,
    locks: Arc<Mutex<BTreeMap<String, Mutex<Option<String>>>>>,

    external_url: String,
}

impl TerraformStateStore {
    pub async fn get(&self, project_id: &str) -> anyhow::Result<Option<String>> {
        let states = self.states.lock().await;

        tracing::debug!(project_id, "get state");

        Ok(states.get(project_id).cloned().flatten())
    }

    pub async fn add(&self, project_id: &str, lock_id: &str, state: &str) -> anyhow::Result<()> {
        let mut states = self.states.lock().await;
        if let Some(lock) = self.get_lock(project_id).await?
            && lock == lock_id
        {
        } else {
            anyhow::bail!("lock id doesn't match the currently held lock for project");
        };

        tracing::debug!(project_id, "saving state");

        states.insert(project_id.into(), Some(state.to_string()));

        Ok(())
    }

    fn state_id(&self, destination: &Destination, project_id: &str) -> String {
        format!("{}.{}", destination.environment, project_id)
    }

    async fn urls(&self, state_id: String) -> (String, String) {
        let mut user = self.users.lock().await;

        let secret = user.entry(state_id.clone()).or_insert_with(|| {
            let id = uuid::Uuid::new_v4();
            id.to_string()
        });

        (state_id, secret.clone())
    }

    async fn get_lock(&self, project_id: &str) -> anyhow::Result<Option<String>> {
        let locks = self.locks.lock().await;
        if let Some(project_handle) = locks.get(project_id)
            && let Some(project) = project_handle.lock().await.as_ref()
        {
            return Ok(Some(project.clone()));
        }

        Ok(None)
    }

    async fn attempt_lock(&self, project_id: String, lock_id: &str) -> anyhow::Result<LockState> {
        let mut locks = self.locks.lock().await;

        let project_lock = locks
            .entry(project_id.to_string())
            .or_insert_with(Mutex::default);

        let mut project_handle = project_lock.lock().await;

        if let Some(project_handle) = project_handle.as_ref() {
            // Same lock is held
            if project_handle == lock_id {
                return Ok(LockState::Held);
            }

            return Ok(LockState::Wait);
        }

        *project_handle = Some(lock_id.to_string());

        Ok(LockState::Held)
    }

    async fn attempt_unlock(
        &self,
        project_id: String,
        lock_id: &str,
    ) -> anyhow::Result<UnlockState> {
        let locks = self.locks.lock().await;

        let Some(project_lock) = locks.get(&project_id) else {
            return Ok(UnlockState::Available);
        };

        let mut project_handle = project_lock.lock().await;

        if let Some(_handle) = project_handle.take_if(|handle| handle == lock_id) {
            return Ok(UnlockState::Available);
        }

        Ok(UnlockState::NotOwnedLock)
    }
}

enum LockState {
    Held,
    Wait,
}
enum UnlockState {
    Available,
    NotOwnedLock,
}

pub trait TerraformStateStoreState {
    fn terraform_state_store(&self) -> TerraformStateStore;
}

impl TerraformStateStoreState for State {
    fn terraform_state_store(&self) -> TerraformStateStore {
        static ONCE: OnceLock<TerraformStateStore> = OnceLock::new();

        ONCE.get_or_init(|| TerraformStateStore {
            states: Arc::default(),
            users: Arc::default(),
            locks: Arc::default(),

            external_url: self
                .config
                .terraform_external_host
                .clone()
                .expect("to be able to get external terraform url"),
        })
        .clone()
    }
}

pub struct TerraformV1Server {
    state: State,
    host: SocketAddr,
}

impl TerraformV1Server {
    pub async fn serve(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let router = axum::Router::new()
            .route("/{project_id}", get(Self::get_state).post(Self::post_state))
            .route("/{project_id}/lock", post(Self::lock_state))
            .route("/{project_id}/unlock", post(Self::unlock_state))
            .layer(TraceLayer::new_for_http().on_request(
                |req: &axum::http::Request<_>, _span: &Span| {
                    let uri = req.uri();
                    let method = req.method();
                    let path = uri.path_and_query();

                    tracing::info!(?method, ?path, "terraform v1 request");
                },
            ))
            .with_state(self.state.clone());

        let listener = TcpListener::bind(self.host).await?;

        tracing::info!(host = %self.host, "starting terraform v1 server");

        axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async move {
                cancel.cancelled().await;
            })
            .await
            .context("terraform v1 server")?;

        Ok(())
    }

    async fn get_state(
        AState(state): AState<State>,
        Path(project_id): Path<String>,
    ) -> Result<impl IntoResponse, ApiError> {
        let Ok(state) = state.terraform_state_store().get(&project_id).await else {
            tracing::info!(project_id, "failed to request state");
            return Err(ApiError::BadRequest);
        };

        let Some(state) = state else {
            tracing::info!(project_id, "no terraform state found");
            return Err(ApiError::NotFound);
        };

        Ok(state)
    }
    async fn post_state(
        AState(state): AState<State>,
        Path(project_id): Path<String>,
        Query(req): Query<LockRequest>,
        body: String,
    ) -> Result<impl IntoResponse, ApiError> {
        if let Err(e) = state
            .terraform_state_store()
            .add(&project_id, &req.lock_id, &body)
            .await
        {
            tracing::error!("failed to save state: {e:#}");
            return Err(ApiError::InternalServerError);
        }

        Ok(())
    }
    async fn lock_state(
        AState(state): AState<State>,
        Path(project_id): Path<String>,
        Json(req): Json<LockRequest>,
    ) -> Result<impl IntoResponse, ApiError> {
        tracing::info!(lock_id = req.lock_id, project_id, "locking state");

        let Ok(lock) = state
            .terraform_state_store()
            .attempt_lock(project_id, &req.lock_id)
            .await
        else {
            tracing::warn!("failed to lock state");
            return Err(ApiError::InternalServerError);
        };

        match lock {
            LockState::Held => Ok(()),
            LockState::Wait => Err(ApiError::Custom(StatusCode::CONFLICT)),
        }
    }
    async fn unlock_state(
        AState(state): AState<State>,
        Path(project_id): Path<String>,
        Json(req): Json<LockRequest>,
    ) -> Result<impl IntoResponse, ApiError> {
        tracing::info!(lock_id = req.lock_id, project_id, "unlocking state");

        let Ok(lock) = state
            .terraform_state_store()
            .attempt_unlock(project_id, &req.lock_id)
            .await
        else {
            tracing::warn!("failed to unlock state");
            return Err(ApiError::InternalServerError);
        };

        match lock {
            UnlockState::Available => Ok(()),
            UnlockState::NotOwnedLock => Err(ApiError::BadRequest),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LockRequest {
    #[serde(alias = "ID")]
    lock_id: String,
}

struct StatePath {}

pub enum ApiError {
    BadRequest,
    NotFound,
    InternalServerError,
    Custom(StatusCode),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        match self {
            ApiError::BadRequest => (StatusCode::BAD_REQUEST, "invalid request"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "found no plan"),
            ApiError::InternalServerError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
            }
            ApiError::Custom(status) => (status, "custom error"),
        }
        .into_response()
    }
}

#[async_trait::async_trait]
impl Component for TerraformV1Server {
    fn name(&self) -> Option<String> {
        Some("forest-server/terraform-v1-server".into())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        self.serve(cancellation_token).await?;

        Ok(())
    }
}

pub trait TerraformV1ServerState {
    fn terraform_v1_server(&self, host: SocketAddr) -> TerraformV1Server;
}

impl TerraformV1ServerState for State {
    fn terraform_v1_server(&self, host: SocketAddr) -> TerraformV1Server {
        TerraformV1Server {
            state: self.clone(),
            host,
        }
    }
}

pub struct TerraformV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
    pub tf_state: TerraformStateStore,
}

impl TerraformV1Destination {
    async fn run(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let project_id = &release.project_id;
        let state_id = self.tf_state.state_id(destination, &project_id.to_string());
        let (id, password) = self.tf_state.urls(state_id).await;

        let base = format!("{}/{id}", self.tf_state.external_url.trim_end_matches("/"));
        let lock = format!("{base}/lock");
        let unlock = format!("{base}/unlock");

        let tf_envs = HashMap::from([
            ("TF_HTTP_ADDRESS", base.as_str()),
            ("TF_HTTP_UNLOCK_ADDRESS", unlock.as_str()),
            ("TF_HTTP_LOCK_ADDRESS", lock.as_str()),
            ("TF_HTTP_PASSWORD", password.as_str()),
        ]);

        let temp_dir = self.temp.create_emphemeral_temp().await?;
        let files = self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
            .context("get files for release")?;

        // 1. Fill temp dir with the correct files
        for (path, content) in files {
            let path = temp_dir.join(path);
            tracing::debug!("placing files in: {}", path.display());

            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("terraform create dir")?;
            }

            let mut file = tokio::fs::File::create_new(path)
                .await
                .context("terraform create file")?;
            file.write_all(content.as_bytes())
                .await
                .context("terraform write content")?;
            file.flush().await.context("terraform flush file")?
        }

        let env_dir = &temp_dir.join(&destination.environment);

        let mut env_dir_entries = tokio::fs::read_dir(env_dir)
            .await
            .context("read dir found no destinations for env")?;

        let mut matched = false;
        while let Some(env_dir_entry) = env_dir_entries.next_entry().await? {
            let entry = env_dir_entry.file_type().await?;
            if !entry.is_dir() {
                // Ignore forest dirs
                continue;
            }

            let entry_name = env_dir_entry.file_name();
            let entry_name = entry_name.to_string_lossy().to_string();
            if let Ok(re) = regex::Regex::new(&entry_name.clone()) {
                if !re.is_match(&destination.name) {
                    tracing::debug!(
                        "destination (regex) is not a match: files: {}, destination_name: {}",
                        entry_name,
                        destination.name
                    );
                    continue;
                }
            } else if entry_name != destination.name {
                tracing::debug!(
                    "destination is not a match: files: {}, destination_name: {}",
                    entry_name,
                    destination.name
                );
                continue;
            }

            matched = true;

            let dir = env_dir
                .join(entry_name) // find name that matches the dir
                .join(&destination.destination_type.organisation)
                .join(format!(
                    "{}@{}",
                    destination.destination_type.name, destination.destination_type.version
                ));

            // 2. Run terraform command over it
            self.run_command(logger, destination, &dir, &tf_envs, &["init"])
                .await
                .context("terraform init")?;

            match mode {
                Mode::Prepare => {
                    tracing::info!("running terraform plan");
                    self.run_command(logger, destination, &dir, &tf_envs, &["plan"])
                        .await
                        .context("terraform plan")?;
                }
                Mode::Apply => {
                    tracing::info!("running terraform apply");
                    self.run_command(
                        logger,
                        destination,
                        &dir,
                        &tf_envs,
                        &["apply", "-auto-approve"],
                    )
                    .await
                    .context("terraform apply")?;
                }
            }
        }

        if !matched {
            anyhow::bail!("failed to find a destination match for submitted release");
        }

        Ok(())
    }

    async fn run_command(
        &self,
        logger: &DestinationLogger,
        destination: &Destination,
        path: &std::path::Path,
        tf_envs: &HashMap<&str, &str>,
        args: &[&str],
    ) -> anyhow::Result<()> {
        tracing::debug!(path =% path.display(), "running terraform {}", args.join(" "));

        let exe = std::env::var("TERRAFORM_EXE").unwrap_or("terraform".to_string());

        let mut cmd = tokio::process::Command::new(exe);
        cmd.current_dir(path)
            .env("NO_COLOR", "1")
            .env("TF_IN_AUTOMATION", "true")
            .env("TF_HTTP_LOCK_METHOD", "POST")
            .env("TF_HTTP_UNLOCK_METHOD", "POST")
            .env("TF_HTTP_USERNAME", "forest-terraform-v1")
            .env("CI", "true")
            .envs(tf_envs);

        for (k, v) in &destination.metadata {
            cmd.env(format!("TF_VAR_{}", k), v);
        }

        let mut proc = cmd
            .args(args)
            .arg("-no-color")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        if let Some(stdout) = proc.stdout.take() {
            let logger = logger.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("terraform@1: {}", line);
                    logger.log_stdout(&line);
                }
            });
        }
        if let Some(stderr) = proc.stderr.take() {
            let logger = logger.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("terraform@1: {}", line);
                    logger.log_stderr(&line);
                }
            });
        }

        let exit = proc.wait().await.context("terraform failed")?;
        if !exit.success() {
            anyhow::bail!("terraform failed: {}", exit.code().unwrap_or(-1));
        }

        tracing::debug!("terraform command success");

        Ok(())
    }
}

#[async_trait::async_trait]
impl DestinationEdge for TerraformV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "terraform".into(),
            version: 1,
        }
    }
    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(logger, release, destination, Mode::Prepare)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(logger, release, destination, Mode::Apply)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }
}

enum Mode {
    Prepare,
    Apply,
}
