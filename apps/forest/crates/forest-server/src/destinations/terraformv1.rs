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
use notmad::{Component, ComponentInfo, MadError};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
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

/// Resolve the terraform-compatible binary to invoke.
///
/// Priority: `TERRAFORM_EXE` env override → first of `terraform` / `tofu`
/// found on PATH → `terraform` (let spawn fail with a clear ENOENT).
///
/// Existed because the production forest-server image ships OpenTofu
/// (`tofu`), not `terraform`, and was missing the `TERRAFORM_EXE=tofu`
/// override — every release failed with `terraform init: No such file
/// or directory`.
fn resolve_terraform_exe() -> String {
    if let Ok(exe) = std::env::var("TERRAFORM_EXE") {
        if !exe.is_empty() {
            return exe;
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for candidate in ["terraform", "tofu"] {
            for dir in path_var.split(':') {
                let candidate_path = std::path::Path::new(dir).join(candidate);
                if candidate_path.is_file() {
                    return candidate.to_string();
                }
            }
        }
    }
    "terraform".to_string()
}

#[derive(Clone)]
pub struct TerraformStateStore {
    // State blobs and per-state-id locks live in postgres so they survive
    // forest-server restarts. Each successful tofu apply appends a new
    // `terraform_states` row; `get()` returns the latest. Locks live in
    // `terraform_state_locks` and are released on unlock or by admin
    // DELETE if a runner crashes mid-apply.
    //
    // The per-state-id basic-auth secret stays in memory — it's
    // regenerated on demand, only matters within a single forest-server
    // process lifetime, and any in-flight tofu session is killed when
    // forest restarts anyway.
    db: PgPool,
    users: Arc<Mutex<BTreeMap<String, String>>>,

    pub external_url: String,
}

impl TerraformStateStore {
    pub async fn get(&self, project_id: &str) -> anyhow::Result<Option<String>> {
        tracing::debug!(project_id, "get state");

        let row = sqlx::query_scalar!(
            r#"
                SELECT content
                FROM terraform_states
                WHERE state_id = $1
                ORDER BY id DESC
                LIMIT 1
            "#,
            project_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("read terraform state")?;

        Ok(row)
    }

    pub async fn add(&self, project_id: &str, lock_id: &str, state: &str) -> anyhow::Result<()> {
        if let Some(lock) = self.get_lock(project_id).await?
            && lock == lock_id
        {
        } else {
            anyhow::bail!("lock id doesn't match the currently held lock for project");
        };

        tracing::debug!(project_id, "saving state");

        sqlx::query!(
            r#"
                INSERT INTO terraform_states (state_id, content)
                VALUES ($1, $2)
            "#,
            project_id,
            state,
        )
        .execute(&self.db)
        .await
        .context("write terraform state")?;

        Ok(())
    }

    pub fn state_id(&self, destination: &Destination, project_id: &str) -> String {
        format!("{}.{}", destination.environment, project_id)
    }

    /// Like [`state_id`](Self::state_id) but for callers that don't have a
    /// `Destination` handy (e.g. the scheduler when computing credentials
    /// for a remote runner). The shape MUST match `state_id` exactly so
    /// in-process and remote runs converge on the same state record.
    pub fn state_id_for(environment: &str, project_id: &str) -> String {
        format!("{environment}.{project_id}")
    }

    /// Look up (or generate) the secret for a state id and return both back.
    pub async fn urls(&self, state_id: String) -> (String, String) {
        let mut user = self.users.lock().await;

        let secret = user.entry(state_id.clone()).or_insert_with(|| {
            let id = uuid::Uuid::new_v4();
            id.to_string()
        });

        (state_id, secret.clone())
    }

    async fn get_lock(&self, project_id: &str) -> anyhow::Result<Option<String>> {
        let row = sqlx::query_scalar!(
            r#"SELECT lock_id FROM terraform_state_locks WHERE state_id = $1"#,
            project_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("read terraform state lock")?;

        Ok(row)
    }

    async fn attempt_lock(&self, project_id: String, lock_id: &str) -> anyhow::Result<LockState> {
        let mut tx = self.db.begin().await.context("begin lock tx")?;

        let existing: Option<String> = sqlx::query_scalar!(
            r#"
                SELECT lock_id
                FROM terraform_state_locks
                WHERE state_id = $1
                FOR UPDATE
            "#,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await
        .context("select state lock for update")?;

        let outcome = match existing.as_deref() {
            Some(holder) if holder == lock_id => LockState::Held,
            Some(_) => LockState::Wait,
            None => {
                sqlx::query!(
                    r#"
                        INSERT INTO terraform_state_locks (state_id, lock_id)
                        VALUES ($1, $2)
                    "#,
                    project_id,
                    lock_id,
                )
                .execute(&mut *tx)
                .await
                .context("insert state lock")?;
                LockState::Held
            }
        };

        tx.commit().await.context("commit lock tx")?;
        Ok(outcome)
    }

    async fn attempt_unlock(
        &self,
        project_id: String,
        lock_id: &str,
    ) -> anyhow::Result<UnlockState> {
        let deleted = sqlx::query!(
            r#"
                DELETE FROM terraform_state_locks
                WHERE state_id = $1 AND lock_id = $2
            "#,
            project_id,
            lock_id,
        )
        .execute(&self.db)
        .await
        .context("delete state lock")?
        .rows_affected();

        if deleted > 0 {
            return Ok(UnlockState::Available);
        }

        // Either no lock exists (already unlocked) or another holder owns it.
        let held_by_other = sqlx::query_scalar!(
            r#"SELECT 1 FROM terraform_state_locks WHERE state_id = $1"#,
            project_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("check state lock holder")?
        .is_some();

        if held_by_other {
            Ok(UnlockState::NotOwnedLock)
        } else {
            Ok(UnlockState::Available)
        }
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
            db: self.db.clone(),
            users: Arc::default(),

            external_url: self.config.terraform_external_host.clone(),
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

impl Component for TerraformV1Server {
    fn info(&self) -> ComponentInfo {
        "forest-server/terraform-v1-server".into()
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

    /// Run terraform init + plan, capturing stdout as the plan output string.
    async fn run_capture(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<String> {
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

        for (path, content) in files {
            let path = temp_dir.join(path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.context("create dir")?;
            }
            let mut file = tokio::fs::File::create_new(path).await.context("create file")?;
            file.write_all(content.as_bytes()).await.context("write")?;
            file.flush().await.context("flush")?;
        }

        let env_dir = &temp_dir.join(&destination.environment);
        let mut env_dir_entries = tokio::fs::read_dir(env_dir)
            .await
            .context("read dir found no destinations for env")?;

        let mut plan_output = String::new();
        let mut matched = false;

        while let Some(env_dir_entry) = env_dir_entries.next_entry().await? {
            if !env_dir_entry.file_type().await?.is_dir() {
                continue;
            }
            let entry_name = env_dir_entry.file_name().to_string_lossy().to_string();
            if let Ok(re) = regex::Regex::new(&entry_name) {
                if !re.is_match(&destination.name) { continue; }
            } else if entry_name != destination.name {
                continue;
            }

            matched = true;
            let dir = env_dir
                .join(&entry_name)
                .join(&destination.destination_type.organisation)
                .join(format!("{}@{}", destination.destination_type.name, destination.destination_type.version));

            // init
            self.run_command(logger, destination, &dir, &tf_envs, &["init"])
                .await
                .context("terraform init")?;

            // plan — capture stdout
            plan_output = self
                .run_command_capture(logger, destination, &dir, &tf_envs, &["plan"])
                .await
                .context("terraform plan")?;
        }

        if !matched {
            anyhow::bail!("failed to find a destination match for submitted release");
        }

        Ok(plan_output)
    }

    /// Like `run_command` but also captures and returns stdout.
    async fn run_command_capture(
        &self,
        logger: &DestinationLogger,
        destination: &Destination,
        path: &std::path::Path,
        tf_envs: &HashMap<&str, &str>,
        args: &[&str],
    ) -> anyhow::Result<String> {
        tracing::debug!(path =% path.display(), "running terraform {} (capture)", args.join(" "));

        let exe = resolve_terraform_exe();

        let mut cmd = tokio::process::Command::new(exe);
        cmd.current_dir(path)
            .env("NO_COLOR", "1")
            .env("TF_IN_AUTOMATION", "true")
            .env("TF_HTTP_LOCK_METHOD", "POST")
            .env("TF_HTTP_UNLOCK_METHOD", "POST")
            .env("TF_HTTP_USERNAME", "forest-terraform-v1")
            .env("CI", "true")
            .envs(tf_envs);

        if let Ok(mirror_url) = std::env::var("FOREST_TERRAFORM_PROVIDER_MIRROR_URL") {
            let cli_config = path.join(".terraformrc");
            tokio::fs::write(
                &cli_config,
                format!("provider_installation {{\n  network_mirror {{\n    url = \"{mirror_url}\"\n  }}\n}}\n"),
            )
            .await
            .ok();
            cmd.env("TF_CLI_CONFIG_FILE", &cli_config);
        }

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

        let captured = Arc::new(tokio::sync::Mutex::new(String::new()));

        if let Some(stdout) = proc.stdout.take() {
            let logger = logger.clone();
            let captured = captured.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("terraform@1: {}", line);
                    logger.log_stdout(&line);
                    let mut buf = captured.lock().await;
                    if !buf.is_empty() { buf.push('\n'); }
                    buf.push_str(&line);
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

        let output = captured.lock().await.clone();
        Ok(output)
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

        let exe = resolve_terraform_exe();

        let mut cmd = tokio::process::Command::new(exe);
        cmd.current_dir(path)
            .env("NO_COLOR", "1")
            .env("TF_IN_AUTOMATION", "true")
            .env("TF_HTTP_LOCK_METHOD", "POST")
            .env("TF_HTTP_UNLOCK_METHOD", "POST")
            .env("TF_HTTP_USERNAME", "forest-terraform-v1")
            .env("CI", "true")
            .envs(tf_envs);

        // Point at a network mirror (pull-through cache) so ephemeral work
        // dirs don't hit GitHub directly for provider downloads.
        if let Ok(mirror_url) = std::env::var("FOREST_TERRAFORM_PROVIDER_MIRROR_URL") {
            let cli_config = path.join(".terraformrc");
            tokio::fs::write(
                &cli_config,
                format!(
                    "provider_installation {{\n  network_mirror {{\n    url = \"{mirror_url}\"\n  }}\n}}\n"
                ),
            )
            .await
            .ok();
            cmd.env("TF_CLI_CONFIG_FILE", &cli_config);
        }

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

    fn description(&self) -> &str {
        "Provision infrastructure with Terraform using an HTTP backend for remote state."
    }

    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        // Terraform passes all metadata keys as TF_VAR_* environment variables.
        // The fields below document the keys consumed by the runner itself; any
        // additional keys are forwarded verbatim to the Terraform configuration.
        vec![
            forest_models::MetadataFieldSchema {
                name: "tf_workspace".into(),
                label: "Terraform Workspace".into(),
                description: "Terraform workspace name. Defaults to the environment name when unset."
                    .into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "tf_parallelism".into(),
                label: "Parallelism".into(),
                description: "Number of concurrent resource operations (terraform -parallelism)."
                    .into(),
                required: false,
                field_type: "number".into(),
                default_value: "10".into(),
            },
        ]
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
            .context("terraform apply failed")?;

        Ok(())
    }

    async fn plan(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Option<String>> {
        let output = self.run_capture(logger, release, destination)
            .await
            .context("terraform plan failed")?;

        Ok(Some(output))
    }

    fn supports_plan(&self) -> bool {
        true
    }
}

enum Mode {
    Prepare,
    Apply,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_terraform_exe_uses_env_override_when_set() {
        // Single test to avoid env-var races between parallel cases.
        let prev = std::env::var("TERRAFORM_EXE").ok();

        unsafe { std::env::set_var("TERRAFORM_EXE", "my-custom-tf"); }
        assert_eq!(resolve_terraform_exe(), "my-custom-tf");

        unsafe { std::env::set_var("TERRAFORM_EXE", ""); }
        // Empty env falls through to PATH search (or "terraform" fallback);
        // never returns the empty string.
        assert!(!resolve_terraform_exe().is_empty());

        match prev {
            Some(v) => unsafe { std::env::set_var("TERRAFORM_EXE", v) },
            None => unsafe { std::env::remove_var("TERRAFORM_EXE") },
        }
    }
}
