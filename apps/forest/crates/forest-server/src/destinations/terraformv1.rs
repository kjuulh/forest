use std::{net::SocketAddr, process::Stdio, sync::Arc};

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
};
use tokio_util::sync::CancellationToken;
use tower_http::trace::TraceLayer;
use tracing::Span;

use crate::{
    State,
    destinations::{
        DestinationEdge, DestinationIndex, DriftReport,
        logger::DestinationLogger,
        terraform_credentials::{self, CredentialKey, Rejection, Scope, Verified},
        terraform_plans::{SavedPlan, TerraformPlanStore},
    },
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
    // Every request to the state server must carry a credential signed for
    // that exact state id (see `terraform_credentials`). Stateless, so it
    // holds across restarts and replicas.
    db: PgPool,
    key: CredentialKey,

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

    /// The read-write credential a release run carries as `TF_HTTP_PASSWORD`.
    ///
    /// Scoped to this state id, named after the destination so the state
    /// server's request log says which destination touched which state, and
    /// sized to a run (`RUNNER_TTL`).
    pub fn runner_credential(&self, state_id: &str, destination: &Destination) -> String {
        self.key.issue(
            state_id,
            Scope::ReadWrite,
            &format!("runner:{}/{}", destination.organisation, destination.name),
            terraform_credentials::RUNNER_TTL,
        )
    }

    /// A credential for an operator. Callers are responsible for having
    /// authorised `principal` against the state's project first.
    pub fn operator_credential(
        &self,
        state_id: &str,
        scope: Scope,
        principal: &str,
        ttl: std::time::Duration,
    ) -> String {
        self.key.issue(
            state_id,
            scope,
            principal,
            ttl.min(terraform_credentials::MAX_OPERATOR_TTL),
        )
    }

    /// The backend URLs for a state id, in the shape terraform's HTTP backend
    /// wants them: (address, lock, unlock).
    pub fn addresses(&self, state_id: &str) -> (String, String, String) {
        let base = format!("{}/{state_id}", self.external_url.trim_end_matches('/'));
        (
            base.clone(),
            format!("{base}/lock"),
            format!("{base}/unlock"),
        )
    }

    fn verify(&self, state_id: &str, headers: &http::HeaderMap) -> Result<Verified, Rejection> {
        let password =
            terraform_credentials::basic_auth_password(headers.get(http::header::AUTHORIZATION))
                .ok_or(Rejection::Malformed)?;
        self.key.verify(state_id, &password)
    }

    /// The holder of a state's lock, if any. Public so an operator path can
    /// report what it is about to break.
    pub async fn lock_holder(&self, state_id: &str) -> anyhow::Result<Option<String>> {
        self.get_lock(state_id).await
    }

    /// Break a state's lock regardless of holder — `terraform force-unlock`
    /// for someone who has no terraform. Returns the lock id removed.
    pub async fn force_unlock(&self, state_id: &str) -> anyhow::Result<Option<String>> {
        let removed = sqlx::query_scalar!(
            r#"DELETE FROM terraform_state_locks WHERE state_id = $1 RETURNING lock_id"#,
            state_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("force-unlock terraform state")?;
        Ok(removed)
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
    // Built per call. It used to be a process-wide `OnceLock`, because the
    // in-memory password map had to be shared; with stateless credentials
    // there is nothing to share, and the singleton pinned whichever pool the
    // first caller happened to hold — which in the test harness is a pool that
    // dies with its test.
    fn terraform_state_store(&self) -> TerraformStateStore {
        TerraformStateStore {
            db: self.db.clone(),
            key: CredentialKey::from_secrets(
                self.config.terraform_state_signing_key.as_deref(),
                &self.config.access_token_secret_key,
            ),

            external_url: self.config.terraform_external_host.clone(),
        }
    }
}

pub struct TerraformV1Server {
    state: State,
    host: SocketAddr,
}

impl TerraformV1Server {
    pub async fn serve(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let router = Self::router(self.state.clone());

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

    /// The state backend's routes, separate from `serve` so tests can drive
    /// them in-process.
    pub fn router(state: State) -> axum::Router {
        axum::Router::new()
            .route("/{state_id}", get(Self::get_state).post(Self::post_state))
            .route("/{state_id}/lock", post(Self::lock_state))
            .route("/{state_id}/unlock", post(Self::unlock_state))
            .layer(TraceLayer::new_for_http().on_request(
                |req: &axum::http::Request<_>, _span: &Span| {
                    let uri = req.uri();
                    let method = req.method();
                    let path = uri.path_and_query();

                    tracing::info!(?method, ?path, "terraform v1 request");
                },
            ))
            .with_state(state)
    }

    /// Every route goes through here first. `write` routes (state POST, lock,
    /// unlock) need a read-write credential; GET takes either.
    ///
    /// Each authorised request is logged with the credential's principal —
    /// that line is the record of who read or wrote which state.
    fn authorise(
        state: &State,
        state_id: &str,
        headers: &http::HeaderMap,
        operation: &'static str,
        write: bool,
    ) -> Result<Verified, ApiError> {
        let verified = match state.terraform_state_store().verify(state_id, headers) {
            Ok(v) => v,
            Err(rejection) => {
                tracing::warn!(
                    state_id,
                    operation,
                    ?rejection,
                    "terraform state request refused: no valid credential for this state"
                );
                return Err(ApiError::Unauthorized);
            }
        };

        if write && verified.scope != Scope::ReadWrite {
            tracing::warn!(
                state_id,
                operation,
                principal = %verified.principal,
                "terraform state request refused: read-only credential"
            );
            return Err(ApiError::Forbidden);
        }

        tracing::info!(
            state_id,
            operation,
            principal = %verified.principal,
            "terraform state request"
        );
        Ok(verified)
    }

    async fn get_state(
        AState(state): AState<State>,
        Path(state_id): Path<String>,
        headers: http::HeaderMap,
    ) -> Result<impl IntoResponse, ApiError> {
        Self::authorise(&state, &state_id, &headers, "get", false)?;

        let Ok(content) = state.terraform_state_store().get(&state_id).await else {
            tracing::info!(state_id, "failed to request state");
            return Err(ApiError::BadRequest);
        };

        let Some(content) = content else {
            tracing::info!(state_id, "no terraform state found");
            return Err(ApiError::NotFound);
        };

        Ok(content)
    }

    async fn post_state(
        AState(state): AState<State>,
        Path(state_id): Path<String>,
        Query(req): Query<LockRequest>,
        headers: http::HeaderMap,
        body: String,
    ) -> Result<impl IntoResponse, ApiError> {
        Self::authorise(&state, &state_id, &headers, "write", true)?;

        if let Err(e) = state
            .terraform_state_store()
            .add(&state_id, &req.lock_id, &body)
            .await
        {
            tracing::error!("failed to save state: {e:#}");
            return Err(ApiError::InternalServerError);
        }

        Ok(())
    }

    async fn lock_state(
        AState(state): AState<State>,
        Path(state_id): Path<String>,
        headers: http::HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<axum::response::Response, ApiError> {
        Self::authorise(&state, &state_id, &headers, "lock", true)?;
        let req = LockRequest::parse(&body)?;
        tracing::info!(lock_id = req.lock_id, state_id, "locking state");

        let store = state.terraform_state_store();
        let Ok(lock) = store.attempt_lock(state_id.clone(), &req.lock_id).await else {
            tracing::warn!("failed to lock state");
            return Err(ApiError::InternalServerError);
        };

        match lock {
            LockState::Held => Ok(StatusCode::OK.into_response()),
            LockState::Wait => {
                // Terraform's HTTP backend reads a 409 body as the current
                // holder's LockInfo, and prints its ID — which is what
                // `terraform force-unlock <ID>` needs. Returning a bare
                // string left an operator with no ID to unlock.
                let holder = store.lock_holder(&state_id).await.ok().flatten();
                Ok((
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "ID": holder.unwrap_or_default() })),
                )
                    .into_response())
            }
        }
    }

    async fn unlock_state(
        AState(state): AState<State>,
        Path(state_id): Path<String>,
        headers: http::HeaderMap,
        body: axum::body::Bytes,
    ) -> Result<impl IntoResponse, ApiError> {
        Self::authorise(&state, &state_id, &headers, "unlock", true)?;
        let req = LockRequest::parse(&body)?;
        tracing::info!(lock_id = req.lock_id, state_id, "unlocking state");

        let Ok(lock) = state
            .terraform_state_store()
            .attempt_unlock(state_id, &req.lock_id)
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

impl LockRequest {
    /// Parsed by hand, after authorisation, rather than with axum's `Json`
    /// extractor: an extractor runs before the handler, so a request with no
    /// credential would otherwise be answered 415/422 — telling an anonymous
    /// caller about the body shape before telling it no.
    fn parse(body: &[u8]) -> Result<Self, ApiError> {
        serde_json::from_slice(body).map_err(|_| ApiError::BadRequest)
    }
}

struct StatePath {}

pub enum ApiError {
    BadRequest,
    NotFound,
    InternalServerError,
    /// No credential, or one not signed for this state, or expired.
    Unauthorized,
    /// A valid read-only credential on a route that writes.
    Forbidden,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        match self {
            ApiError::BadRequest => (StatusCode::BAD_REQUEST, "invalid request"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "found no plan"),
            ApiError::InternalServerError => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
            }
            ApiError::Unauthorized => {
                return (
                    StatusCode::UNAUTHORIZED,
                    [(
                        http::header::WWW_AUTHENTICATE,
                        "Basic realm=\"forest-terraform-v1\"",
                    )],
                    "a credential for this state is required",
                )
                    .into_response();
            }
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "this credential is read-only"),
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
    pub plans: TerraformPlanStore,
}

/// The directories this release should run terraform in, for this destination.
///
/// `release prepare` gives every rendered deployment item a directory of its
/// own — `{env}/{selector}/{type}` — and drops that item's `forest/config.json`
/// inside it. Reading those records is the only reliable way back from the
/// uploaded tree to a directory: the selector is a regex, it routinely contains
/// a `/` (`^dev/.*$`), and a path with a `/` in one of its components is not
/// recoverable by listing one level.
///
/// Listing one level is what this did, and it is the second half of `fungus`'s
/// failed release: `^dev/.*$` lands on disk as `^dev/` then `.*$/`, `read_dir`
/// of the environment sees only `^dev`, and joining the destination's type onto
/// that names a directory nobody ever wrote. `tokio::process` reports a missing
/// working directory as a spawn failure, so it surfaced as `terraform init: No
/// such file or directory (os error 2)` — indistinguishable, from the release
/// view, from a missing terraform binary. Selectors with no `/` in them
/// (`hetzner-dev`) resolved fine, which is why this went unnoticed.
///
/// The item is chosen by `destination_selector::selects`, the same rule that
/// scheduled the release — so a destination cannot be sent work that a different
/// item declared, and a terraform-typed item cannot claim an ECS destination's
/// directory.
///
/// An artifact carrying no item records at all falls back to the original scan.
/// That is the compatibility hinge: those are artifacts prepared before item
/// records existed, and they are exactly the ones whose selectors held no `/`.
async fn work_dirs(
    temp_dir: &std::path::Path,
    destination: &Destination,
    files: &[(std::path::PathBuf, String)],
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    use crate::services::destination_selector;

    let destination_type = destination.destination_type.qualified();

    let items = match destination_selector::parse_items(files) {
        Ok(items) => items,
        Err(e) => {
            // Degrade rather than fail: a release that cannot read its own item
            // records should still get the behaviour it had before they existed.
            tracing::warn!("could not read this release's deployment items: {e:#}");
            Vec::new()
        }
    };

    if !items.is_empty() {
        let mut dirs: Vec<std::path::PathBuf> = items
            .iter()
            .filter(|parsed| parsed.item.env == destination.environment)
            .filter(|parsed| {
                destination_selector::selects(&parsed.item, &destination.name, &destination_type)
            })
            .map(|parsed| temp_dir.join(&parsed.root))
            .collect();

        // `parse_items` orders deepest-root-first for attribution; terraform
        // wants a stable order and does not care which.
        dirs.sort();
        dirs.dedup();

        return Ok(dirs);
    }

    legacy_work_dirs(temp_dir, destination).await
}

/// The pre-item-record scan, kept for artifacts that carry no `forest/config.json`.
///
/// One level under `{env}/`, each entry treated as a regex over the destination
/// name, the destination's own type joined on. Correct only while no selector
/// contains a `/`; see `work_dirs`.
async fn legacy_work_dirs(
    temp_dir: &std::path::Path,
    destination: &Destination,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let env_dir = temp_dir.join(&destination.environment);

    let mut entries = tokio::fs::read_dir(&env_dir)
        .await
        .context("read dir found no destinations for env")?;

    let mut dirs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            // Ignore forest dirs
            continue;
        }

        let entry_name = entry.file_name().to_string_lossy().to_string();
        if let Ok(re) = regex::Regex::new(&entry_name) {
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

        dirs.push(
            env_dir
                .join(&entry_name)
                .join(&destination.destination_type.organisation)
                .join(format!(
                    "{}@{}",
                    destination.destination_type.name, destination.destination_type.version
                )),
        );
    }

    dirs.sort();
    Ok(dirs)
}

/// A release's artifact laid out on disk, and the terraform roots in it that
/// belong to this destination. Holding `_temp` keeps the directory alive.
struct Staged {
    _temp: crate::temp_dir::GuardedTempDirectory,
    root: std::path::PathBuf,
    dirs: Vec<std::path::PathBuf>,
}

/// A work dir's path relative to the temp root: the key a saved plan is
/// stored under, identical between the plan stage and the deploy stage because
/// both lay out the same artifact.
fn root_key(root: &std::path::Path, dir: &std::path::Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .to_string_lossy()
        .into_owned()
}

/// The file `plan -out` writes and `apply` reads, inside each work dir.
const PLAN_FILE: &str = "forest.tfplan";
const LOCK_FILE: &str = ".terraform.lock.hcl";

impl TerraformV1Destination {
    /// The HTTP-backend environment for this release's state.
    async fn backend_env(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> Vec<(&'static str, String)> {
        let state_id = self
            .tf_state
            .state_id(destination, &release.project_id.to_string());
        let password = self.tf_state.runner_credential(&state_id, destination);
        let (base, lock, unlock) = self.tf_state.addresses(&state_id);
        vec![
            ("TF_HTTP_LOCK_ADDRESS", lock),
            ("TF_HTTP_UNLOCK_ADDRESS", unlock),
            ("TF_HTTP_ADDRESS", base),
            ("TF_HTTP_PASSWORD", password),
        ]
    }

    /// Write the release's files into a fresh temp dir and find this
    /// destination's roots in it.
    async fn stage(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Staged> {
        let temp = self.temp.create_emphemeral_temp().await?;
        let root: std::path::PathBuf = (*temp).clone();
        let files = self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
            .context("get files for release")?;

        for (path, content) in &files {
            let path = root.join(path);
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

        let dirs = work_dirs(&root, destination, &files).await?;
        if dirs.is_empty() {
            anyhow::bail!("failed to find a destination match for submitted release");
        }

        Ok(Staged {
            _temp: temp,
            root,
            dirs,
        })
    }

    /// Plan every root, keep each binary plan, and return the text a reviewer
    /// approves — every root's, not just the last one's.
    async fn plan_and_save(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<String> {
        let destroy = self.plans.is_destroy_plan(release.id).await?;
        let env = self.backend_env(release, destination).await;
        let staged = self.stage(release, destination).await?;
        let (outputs, saved) = plan_roots(
            logger,
            destination,
            &env,
            &staged.root,
            &staged.dirs,
            destroy,
        )
        .await?;

        self.plans
            .save(release.release_intent_id, release.destination_id, &saved)
            .await
            .context("save plans")?;

        let output = join_plan_outputs(outputs);
        if destroy {
            // Said before the plan so it cannot be missed by someone skimming
            // for "Plan:" at the bottom.
            return Ok(format!(
                "⚠ DESTROY PLAN — approving this removes everything shown below, and the deploy \
                 stage applies it as-is.\n\n{output}"
            ));
        }
        Ok(output)
    }

    /// Apply what the plan stage saved, root for root. Refuses a root with no
    /// saved plan rather than planning it afresh.
    async fn apply_saved(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        saved: Vec<SavedPlan>,
    ) -> anyhow::Result<()> {
        let env = self.backend_env(release, destination).await;
        let staged = self.stage(release, destination).await?;
        apply_roots(
            logger,
            destination,
            &env,
            &staged.root,
            &staged.dirs,
            &saved,
        )
        .await
    }

    /// The pre-#284 path, for releases that never had a plan stage: plan and
    /// apply in one go, as a direct `forest release` always has.
    async fn run_unplanned(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let env = self.backend_env(release, destination).await;
        let staged = self.stage(release, destination).await?;

        for dir in &staged.dirs {
            run_command(logger, destination, dir, &env, &["init", "-input=false"])
                .await
                .context("terraform init")?;

            match mode {
                Mode::Prepare => {
                    tracing::info!("running terraform plan");
                    run_command(logger, destination, dir, &env, &["plan", "-input=false"])
                        .await
                        .context("terraform plan")?;
                }
                Mode::Apply => {
                    tracing::info!("running terraform apply");
                    run_command(
                        logger,
                        destination,
                        dir,
                        &env,
                        &["apply", "-input=false", "-auto-approve"],
                    )
                    .await
                    .context("terraform apply")?;
                }
            }
        }

        Ok(())
    }
}

/// Plan every root with `-out`, returning each root's text and binary plan.
async fn plan_roots(
    logger: &DestinationLogger,
    destination: &Destination,
    env: &[(&str, String)],
    root: &std::path::Path,
    dirs: &[std::path::PathBuf],
    destroy: bool,
) -> anyhow::Result<(Vec<(String, String)>, Vec<SavedPlan>)> {
    let mut plan_args = vec!["plan", "-input=false", "-out", PLAN_FILE];
    if destroy {
        plan_args.push("-destroy");
    }

    let mut outputs = Vec::new();
    let mut saved = Vec::new();
    for dir in dirs {
        run_command(logger, destination, dir, env, &["init", "-input=false"])
            .await
            .context("terraform init")?;

        let output = run_command_capture(logger, destination, dir, env, &plan_args)
            .await
            .context("terraform plan")?;

        let plan = tokio::fs::read(dir.join(PLAN_FILE))
            .await
            .context("read the saved plan")?;
        let lock_file = tokio::fs::read_to_string(dir.join(LOCK_FILE)).await.ok();

        let key = root_key(root, dir);
        outputs.push((key.clone(), output));
        saved.push(SavedPlan {
            work_dir: key,
            plan,
            lock_file,
        });
    }

    Ok((outputs, saved))
}

/// Apply what the plan stage saved, root for root. Refuses a root with no
/// saved plan rather than planning it afresh.
async fn apply_roots(
    logger: &DestinationLogger,
    destination: &Destination,
    env: &[(&str, String)],
    root: &std::path::Path,
    dirs: &[std::path::PathBuf],
    saved: &[SavedPlan],
) -> anyhow::Result<()> {
    let missing: Vec<String> = dirs
        .iter()
        .map(|dir| root_key(root, dir))
        .filter(|key| !saved.iter().any(|p| &p.work_dir == key))
        .collect();
    if !missing.is_empty() {
        anyhow::bail!(
            "the plan stage saved no plan for {} — refusing to apply what nobody reviewed. \
             Re-run the release so the plan stage covers every root",
            missing.join(", ")
        );
    }

    for dir in dirs {
        let key = root_key(root, dir);
        let plan = saved
            .iter()
            .find(|p| p.work_dir == key)
            .expect("checked above");

        // The providers the plan was made with, not whatever `init` would
        // resolve today: terraform refuses a plan whose providers changed
        // underneath it.
        if let Some(lock) = &plan.lock_file {
            tokio::fs::write(dir.join(LOCK_FILE), lock)
                .await
                .context("restore lock file")?;
        }
        tokio::fs::write(dir.join(PLAN_FILE), &plan.plan)
            .await
            .context("restore saved plan")?;

        run_command(logger, destination, dir, env, &["init", "-input=false"])
            .await
            .context("terraform init")?;

        // No -auto-approve: a saved plan is already approved, and terraform
        // refuses it outright if the state moved since ("Saved plan is
        // stale").
        run_command(
            logger,
            destination,
            dir,
            env,
            &["apply", "-input=false", PLAN_FILE],
        )
        .await
        .with_context(|| {
            format!(
                "terraform apply of the approved plan for {key} — if terraform reports the \
                     plan as stale, the state changed after approval: re-run the release and \
                     review the new plan"
            )
        })?;
    }

    Ok(())
}

/// Like `run_command` but also captures and returns stdout.
async fn run_command_capture(
    logger: &DestinationLogger,
    destination: &Destination,
    path: &std::path::Path,
    tf_envs: &[(&str, String)],
    args: &[&str],
) -> anyhow::Result<String> {
    let (code, output) = run_command_status(logger, destination, path, tf_envs, args).await?;
    if code != 0 {
        anyhow::bail!("terraform failed: {code}");
    }
    Ok(output)
}

/// Run terraform, capture stdout, and hand back the exit code rather than
/// failing on a nonzero one — for `-detailed-exitcode`, where 2 is an answer.
async fn run_command_status(
    logger: &DestinationLogger,
    destination: &Destination,
    path: &std::path::Path,
    tf_envs: &[(&str, String)],
    args: &[&str],
) -> anyhow::Result<(i32, String)> {
    tracing::debug!(path =% path.display(), "running terraform {} (capture)", args.join(" "));

    let mut cmd = command(destination, path, tf_envs).await;

    let mut proc = cmd
        .args(args)
        .arg("-no-color")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let captured = Arc::new(tokio::sync::Mutex::new(String::new()));

    let stdout_task = proc.stdout.take().map(|stdout| {
        let logger = logger.clone();
        let captured = captured.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("terraform@1: {}", line);
                logger.log_stdout(&line);
                let mut buf = captured.lock().await;
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&line);
            }
        })
    });
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
    // The reader may still be draining the pipe when the process exits;
    // without this the captured plan can lose its last lines.
    if let Some(task) = stdout_task {
        let _ = task.await;
    }

    let output = captured.lock().await.clone();
    Ok((exit.code().unwrap_or(-1), output))
}

/// The drift check, per root: refresh-only, so it asks "did the world move
/// away from state?" rather than "does config differ from state?"; unlocked,
/// because it writes nothing and must not queue behind — or block — a release.
async fn drift_roots(
    logger: &DestinationLogger,
    destination: &Destination,
    env: &[(&str, String)],
    root: &std::path::Path,
    dirs: &[std::path::PathBuf],
) -> anyhow::Result<Vec<DriftReport>> {
    let args = [
        "plan",
        "-refresh-only",
        "-detailed-exitcode",
        "-lock=false",
        "-input=false",
    ];

    let mut reports = Vec::new();
    for dir in dirs {
        run_command(logger, destination, dir, env, &["init", "-input=false"])
            .await
            .context("terraform init")?;

        let (code, output) = run_command_status(logger, destination, dir, env, &args)
            .await
            .context("terraform plan -refresh-only")?;

        let drifted = match code {
            0 => false,
            2 => true,
            other => anyhow::bail!(
                "terraform plan -refresh-only failed ({other}) for {}:\n{output}",
                root_key(root, dir)
            ),
        };
        reports.push(DriftReport {
            work_dir: root_key(root, dir),
            drifted,
            output,
        });
    }
    Ok(reports)
}

async fn run_command(
    logger: &DestinationLogger,
    destination: &Destination,
    path: &std::path::Path,
    tf_envs: &[(&str, String)],
    args: &[&str],
) -> anyhow::Result<()> {
    tracing::debug!(path =% path.display(), "running terraform {}", args.join(" "));

    let mut cmd = command(destination, path, tf_envs).await;

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

/// The environment every terraform invocation gets. One builder for both
/// the capturing and the streaming path, which were two near-identical
/// copies — and the AWS profile has to be in both, or init and plan would
/// authenticate differently.
async fn command(
    destination: &Destination,
    path: &std::path::Path,
    tf_envs: &[(&str, String)],
) -> tokio::process::Command {
    let exe = resolve_terraform_exe();

    let mut cmd = tokio::process::Command::new(exe);
    cmd.current_dir(path)
        .env("NO_COLOR", "1")
        .env("TF_IN_AUTOMATION", "true")
        .env("TF_HTTP_LOCK_METHOD", "POST")
        .env("TF_HTTP_UNLOCK_METHOD", "POST")
        .env("TF_HTTP_USERNAME", "forest-terraform-v1")
        .env("CI", "true");
    for (k, v) in tf_envs {
        cmd.env(k, v);
    }

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

    // Before the TF_VAR pass, so a destination naming a role gets one.
    if let Some(aws_config) = write_aws_profile(path, destination).await {
        cmd.env("AWS_CONFIG_FILE", &aws_config)
            .env("AWS_PROFILE", "forest")
            // A stale AWS_PROFILE in forest-server's own environment would
            // otherwise win over the file we just wrote.
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env_remove("AWS_SESSION_TOKEN");
    }

    for (k, v) in &destination.metadata {
        cmd.env(format!("TF_VAR_{}", k), v);
    }

    cmd
}

/// Every root's plan, labelled when there is more than one. Before #284 only
/// the last root's output survived the loop, so a destination with two roots
/// showed half its plan for approval.
fn join_plan_outputs(outputs: Vec<(String, String)>) -> String {
    if outputs.len() == 1 {
        return outputs
            .into_iter()
            .next()
            .map(|(_, o)| o)
            .unwrap_or_default();
    }
    outputs
        .into_iter()
        .map(|(dir, output)| format!("── {dir} ──\n{output}"))
        .collect::<Vec<_>>()
        .join("\n\n")
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
                description:
                    "Terraform workspace name. Defaults to the environment name when unset.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "tf_parallelism".into(),
                label: "Parallelism".into(),
                description: "Number of concurrent resource operations (terraform -parallelism)."
                    .into(),
                required: false,
                field_type: "number".into(),
                default_value: "10".into(),
                sensitive: false,
            },
        ]
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        // A deploy stage whose plan stage saved a plan has nothing to prepare:
        // planning again here would print a plan into the log that is not the
        // one about to be applied.
        if !self
            .plans
            .load(release.release_intent_id, release.destination_id)
            .await?
            .is_empty()
        {
            logger.log_stdout("forest: applying the plan approved in this release's plan stage");
            return Ok(());
        }

        self.run_unplanned(logger, release, destination, Mode::Prepare)
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
        let (intent, dest) = (release.release_intent_id, release.destination_id);
        let saved = self.plans.load(intent, dest).await?;

        if saved.is_empty() {
            if self.plans.plan_stage_ran(intent, dest).await? {
                anyhow::bail!(
                    "this release's plan stage ran against this destination, but there is no saved \
                     plan to apply (already applied, pruned, or made before forest#284). Refusing to \
                     re-plan and apply something nobody approved — re-run the release"
                );
            }

            return self
                .run_unplanned(logger, release, destination, Mode::Apply)
                .await
                .context("terraform apply failed");
        }

        let result = self.apply_saved(logger, release, destination, saved).await;

        // Spent if it applied, stale if it did not: either way it must not be
        // applied again, and it holds secrets.
        if let Err(e) = self.plans.discard(intent, dest).await {
            tracing::warn!("could not discard applied plans: {e:#}");
        }

        result.context("terraform apply failed")
    }

    async fn plan(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Option<String>> {
        let output = self
            .plan_and_save(logger, release, destination)
            .await
            .context("terraform plan failed")?;

        Ok(Some(output))
    }

    fn supports_plan(&self) -> bool {
        true
    }

    async fn drift(
        &self,
        logger: &DestinationLogger,
        deployed: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Option<Vec<DriftReport>>> {
        let env = self.backend_env(deployed, destination).await;
        let staged = self.stage(deployed, destination).await?;
        let reports = drift_roots(logger, destination, &env, &staged.root, &staged.dirs).await?;
        Ok(Some(reports))
    }
}

enum Mode {
    Prepare,
    Apply,
}

/// Teach the AWS SDK to assume a role, when the destination names one.
///
/// A destination carrying `aws_role_arn` gets a generated AWS config profile in
/// the run directory and `AWS_CONFIG_FILE` / `AWS_PROFILE` pointing at it. The
/// terraform-aws provider then does the assume itself, through the SDK's normal
/// credential resolution, and no component has to know about any of it.
///
/// Doing it here rather than in each component's `provider "aws"` block is the
/// point. A provider-level `assume_role` works, but it has to be added to every
/// component and kept consistent across them, and a component that forgets it
/// silently falls back to whatever standing credential the destination carries.
/// One runner, one mechanism, every component.
///
/// A profile rather than injected `AWS_SESSION_TOKEN`: the SDK re-assumes when
/// the session expires, so an apply that outlives the role's session duration
/// keeps working. Injecting static session credentials would strand a long
/// apply — an ACM validation wait is exactly the case that gets close.
///
/// Where the *base* credentials come from depends on what else the destination
/// has. Static keys become a source profile; otherwise the SDK is pointed at
/// the ECS task role forest-server itself runs as, which is the shape worth
/// wanting — nothing standing anywhere.
async fn write_aws_profile(
    path: &std::path::Path,
    destination: &Destination,
) -> Option<std::path::PathBuf> {
    let role_arn = destination
        .metadata
        .get("aws_role_arn")
        .map(String::as_str)
        .unwrap_or_default();
    if role_arn.is_empty() {
        return None;
    }

    let region = destination
        .metadata
        .get("aws_region")
        .map(String::as_str)
        .unwrap_or("eu-west-1");

    // Names the deploy in CloudTrail. Truncated because STS rejects a session
    // name over 64 characters, and that arrives as an opaque AccessDenied on
    // the assume rather than as a validation error.
    let mut session_name = format!("forest-{}", destination.environment);
    session_name.truncate(64);
    let session_name: String = session_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "+=,.@-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();

    let access_key = destination
        .metadata
        .get("aws_access_key_id")
        .map(String::as_str)
        .unwrap_or_default();
    let secret_key = destination
        .metadata
        .get("aws_secret_access_key")
        .map(String::as_str)
        .unwrap_or_default();

    // Built line by line and joined, not as one format! with continuations.
    // An AWS config file must have no leading whitespace on its keys, and a
    // multi-line string literal is exactly where stray indentation creeps in.
    let mut lines: Vec<String> = Vec::new();

    if !access_key.is_empty() && !secret_key.is_empty() {
        // Role chaining from the destination's own keys. Transitional: it lets
        // a destination move to a role before its keys are removed.
        lines.push("[profile forest-source]".into());
        lines.push(format!("aws_access_key_id = {access_key}"));
        lines.push(format!("aws_secret_access_key = {secret_key}"));
        lines.push(String::new());
        lines.push("[profile forest]".into());
        lines.push(format!("role_arn = {role_arn}"));
        lines.push("source_profile = forest-source".into());
    } else {
        // `EcsContainer` because forest-server runs as an ECS task; the SDK
        // reads the task role from the container credentials endpoint and
        // assumes from there. No standing credential is involved at any point.
        lines.push("[profile forest]".into());
        lines.push(format!("role_arn = {role_arn}"));
        lines.push("credential_source = EcsContainer".into());
    }

    lines.push(format!("role_session_name = {session_name}"));
    lines.push(format!("region = {region}"));

    let config = format!("{}\n", lines.join("\n"));

    let config_path = path.join(".aws-config");
    if let Err(e) = tokio::fs::write(&config_path, config).await {
        // Deliberately not fatal here: the apply fails on its own with a
        // credentials error, which is a clearer signal than this one, and
        // failing here would also mask a destination that never needed the
        // profile.
        tracing::error!(
            error = %e,
            path = %config_path.display(),
            "could not write the AWS profile for the assumed role"
        );
        return None;
    }

    // 0600. The file holds a role ARN and, in the transitional case, the
    // destination's access keys.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            tokio::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).await;
    }

    tracing::debug!(
        role_arn = %role_arn,
        chained = !access_key.is_empty(),
        "terraform will assume a role"
    );

    Some(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forest_models::DestinationType;
    use std::collections::HashMap;

    fn destination_with(metadata: &[(&str, &str)]) -> Destination {
        Destination {
            organisation: "understory".into(),
            name: "dev/eu-west-1/infrastructure-platform".into(),
            environment: "platform-dev".into(),
            metadata: metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            sensitive_keys: vec![],
            destination_type: DestinationType {
                organisation: "forest".into(),
                name: "terraform".into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            },
        }
    }

    #[tokio::test]
    async fn no_role_arn_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = destination_with(&[("aws_region", "eu-west-1")]);

        assert!(write_aws_profile(dir.path(), &dest).await.is_none());
        assert!(
            !dir.path().join(".aws-config").exists(),
            "a destination without a role must be left entirely alone"
        );
    }

    #[tokio::test]
    async fn an_empty_role_arn_is_the_same_as_none() {
        // Metadata keys are strings and a cleared one arrives empty rather
        // than absent, so this is the shape a migrated-then-reverted
        // destination actually has.
        let dir = tempfile::tempdir().unwrap();
        let dest = destination_with(&[("aws_role_arn", "")]);

        assert!(write_aws_profile(dir.path(), &dest).await.is_none());
    }

    #[tokio::test]
    async fn a_role_alone_assumes_from_the_task_role() {
        let dir = tempfile::tempdir().unwrap();
        let dest = destination_with(&[
            (
                "aws_role_arn",
                "arn:aws:iam::462774209206:role/ForestTerraformDeploy",
            ),
            ("aws_region", "eu-west-1"),
        ]);

        let path = write_aws_profile(dir.path(), &dest)
            .await
            .expect("a profile");
        let config = std::fs::read_to_string(&path).unwrap();

        assert!(config.contains("[profile forest]"));
        assert!(config.contains("role_arn = arn:aws:iam::462774209206:role/ForestTerraformDeploy"));
        // The whole point: no standing credential anywhere in the chain.
        assert!(
            config.contains("credential_source = EcsContainer"),
            "without static keys the base credentials must be the task role, got:\n{config}"
        );
        assert!(!config.contains("aws_secret_access_key"));
        assert!(config.contains("region = eu-west-1"));
    }

    #[tokio::test]
    async fn static_keys_become_a_source_profile_for_the_role() {
        let dir = tempfile::tempdir().unwrap();
        let dest = destination_with(&[
            (
                "aws_role_arn",
                "arn:aws:iam::618060699933:role/ForestTerraformDeploy",
            ),
            ("aws_access_key_id", "AKIAEXAMPLE"),
            ("aws_secret_access_key", "shhh"),
        ]);

        let path = write_aws_profile(dir.path(), &dest)
            .await
            .expect("a profile");
        let config = std::fs::read_to_string(&path).unwrap();

        // Role chaining, so a destination can move to a role before its keys
        // are removed rather than in the same change.
        assert!(config.contains("[profile forest-source]"));
        assert!(config.contains("source_profile = forest-source"));
        assert!(config.contains("aws_access_key_id = AKIAEXAMPLE"));
        assert!(!config.contains("credential_source"));
    }

    #[tokio::test]
    async fn the_session_name_is_bounded_and_sts_safe() {
        let dir = tempfile::tempdir().unwrap();
        let mut dest = destination_with(&[("aws_role_arn", "arn:aws:iam::1:role/r")]);
        // Environments are free-form strings; a long one with characters STS
        // rejects would fail the assume with an opaque AccessDenied.
        dest.environment = format!("platform/prod {}", "x".repeat(120));

        let path = write_aws_profile(dir.path(), &dest)
            .await
            .expect("a profile");
        let config = std::fs::read_to_string(&path).unwrap();
        let line = config
            .lines()
            .find(|l| l.starts_with("role_session_name = "))
            .expect("a session name");
        let name = line.trim_start_matches("role_session_name = ");

        assert!(name.len() <= 64, "STS caps this at 64, got {}", name.len());
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+=,.@-_".contains(c)),
            "unexpected character in {name}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_profile_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dest = destination_with(&[
            ("aws_role_arn", "arn:aws:iam::1:role/r"),
            ("aws_access_key_id", "AKIAEXAMPLE"),
            ("aws_secret_access_key", "shhh"),
        ]);

        let path = write_aws_profile(dir.path(), &dest)
            .await
            .expect("a profile");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the file can hold access keys");
    }

    #[test]
    fn resolve_terraform_exe_uses_env_override_when_set() {
        // Single test to avoid env-var races between parallel cases — and it
        // holds TERRAFORM_EXE_LOCK, because the saved-plan tests below spawn
        // the real binary and would otherwise resolve `my-custom-tf` mid-run.
        let _exe = TERRAFORM_EXE_LOCK.blocking_lock();
        let prev = std::env::var("TERRAFORM_EXE").ok();

        unsafe {
            std::env::set_var("TERRAFORM_EXE", "my-custom-tf");
        }
        assert_eq!(resolve_terraform_exe(), "my-custom-tf");

        unsafe {
            std::env::set_var("TERRAFORM_EXE", "");
        }
        // Empty env falls through to PATH search (or "terraform" fallback);
        // never returns the empty string.
        assert!(!resolve_terraform_exe().is_empty());

        match prev {
            Some(v) => unsafe { std::env::set_var("TERRAFORM_EXE", v) },
            None => unsafe { std::env::remove_var("TERRAFORM_EXE") },
        }
    }

    fn destination(name: &str, environment: &str, type_name: &str) -> Destination {
        Destination::new(
            "understory",
            name,
            environment,
            HashMap::new(),
            forest_models::DestinationType {
                organisation: "forest".into(),
                name: type_name.into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            },
        )
    }

    fn item_record(
        env: &str,
        selector: &str,
        destination_type: &str,
    ) -> (std::path::PathBuf, String) {
        (
            std::path::PathBuf::from(format!(
                "{env}/{selector}/{destination_type}/forest/config.json"
            )),
            serde_json::json!({
                "env": env,
                "destination": selector,
                "destination_type": destination_type,
                "config": {},
            })
            .to_string(),
        )
    }

    /// `fungus`'s selector, and the reason its release said `terraform init: No
    /// such file or directory (os error 2)`.
    ///
    /// `^dev/.*$` lands on disk as two directories. Listing one level under the
    /// environment sees `^dev`, and joining the destination's type onto that
    /// names `platform-dev/^dev/forest/terraform@1` — a directory nobody wrote.
    /// The item's own record says where it really is.
    #[tokio::test]
    async fn a_selector_containing_a_slash_still_resolves_to_its_directory() {
        let temp = std::path::Path::new("/tmp/forest-work-dirs-test");
        let files = vec![item_record(
            "platform-dev",
            "^dev/.*$",
            "forest/terraform@1",
        )];

        let dirs = work_dirs(
            temp,
            &destination(
                "dev/eu-west-1/infrastructure-platform",
                "platform-dev",
                "terraform",
            ),
            &files,
        )
        .await
        .expect("resolve");

        assert_eq!(
            dirs,
            vec![temp.join("platform-dev/^dev/.*$/forest/terraform@1")],
        );
    }

    /// And the type is consulted here too, so a terraform destination cannot be
    /// handed the directory an ECS item rendered.
    #[tokio::test]
    async fn an_item_of_another_type_does_not_supply_a_directory() {
        let temp = std::path::Path::new("/tmp/forest-work-dirs-test");
        let files = vec![item_record("platform-dev", ".*", "forest/generic@1")];

        let dirs = work_dirs(
            temp,
            &destination(
                "dev/eu-west-1/infrastructure-platform",
                "platform-dev",
                "terraform",
            ),
            &files,
        )
        .await
        .expect("resolve");

        assert!(dirs.is_empty(), "got: {dirs:?}");
    }

    /// A project declaring both kinds gets the terraform one, and only it.
    #[tokio::test]
    async fn the_right_item_of_several_supplies_the_directory() {
        let temp = std::path::Path::new("/tmp/forest-work-dirs-test");
        let files = vec![
            item_record("platform-dev", ".*", "forest/generic@1"),
            item_record("platform-dev", "^dev/.*$", "forest/terraform@1"),
        ];

        let dirs = work_dirs(
            temp,
            &destination(
                "dev/eu-west-1/infrastructure-platform",
                "platform-dev",
                "terraform",
            ),
            &files,
        )
        .await
        .expect("resolve");

        assert_eq!(
            dirs,
            vec![temp.join("platform-dev/^dev/.*$/forest/terraform@1")],
        );
    }

    /// Items scoped to another environment are not this destination's.
    #[tokio::test]
    async fn an_item_for_another_environment_is_ignored() {
        let temp = std::path::Path::new("/tmp/forest-work-dirs-test");
        let files = vec![item_record("platform-prod", ".*", "forest/terraform@1")];

        let dirs = work_dirs(
            temp,
            &destination("dev/eu-west-1/x", "platform-dev", "terraform"),
            &files,
        )
        .await
        .expect("resolve");

        assert!(dirs.is_empty(), "got: {dirs:?}");
    }
    // ── saved plans (forest#284) ────────────────────────────────────────
    //
    // These drive a real tofu/terraform against a *local* backend and the
    // built-in `terraform_data` resource, so they need no provider download,
    // no network and no forest state server. They fail, not skip, without a
    // binary: CI installs the pinned OpenTofu for them.

    /// Held by every test that reads or writes `TERRAFORM_EXE`. The env is
    /// process-wide and tests run in parallel.
    static TERRAFORM_EXE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn require_terraform() {
        let exe = resolve_terraform_exe();
        let found = std::process::Command::new(&exe)
            .arg("version")
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(
            found,
            "these tests need `tofu` or `terraform` on PATH (CI installs OpenTofu 1.12.6)"
        );
    }

    async fn test_logger() -> DestinationLogger {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL — see apps/forest/CLAUDE.md");
        let db = sqlx::PgPool::connect(&url).await.expect("connect");
        DestinationLogger::new(
            ReleaseItem {
                id: uuid::Uuid::now_v7(),
                release_intent_id: uuid::Uuid::now_v7(),
                artifact: uuid::Uuid::now_v7(),
                project_id: uuid::Uuid::now_v7(),
                destination_id: uuid::Uuid::now_v7(),
                status: "RUNNING".into(),
                project: "saved-plan-test".into(),
            },
            crate::services::release_logs_registry::ReleaseLogsRegistry::new(db),
        )
    }

    /// A release laid out the way `stage` lays one out: one root under a
    /// fresh temp dir, every root sharing `state_dir` as its backend so a plan
    /// and a later apply in a *different* directory see the same state — as
    /// the plan stage and deploy stage do through forest's HTTP backend.
    fn lay_out(
        state_dir: &std::path::Path,
        input: &str,
    ) -> (tempfile::TempDir, Vec<std::path::PathBuf>) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("dev/^dev/x$/forest/terraform@1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main.tf"),
            format!(
                r#"
terraform {{
  backend "local" {{
    path = "{}/terraform.tfstate"
  }}
}}

resource "terraform_data" "x" {{
  input = "{input}"
}}

output "o" {{
  value = terraform_data.x.output
}}
"#,
                state_dir.display()
            ),
        )
        .unwrap();
        (root, vec![dir])
    }

    fn state_output(state_dir: &std::path::Path) -> Option<String> {
        let state = std::fs::read_to_string(state_dir.join("terraform.tfstate")).ok()?;
        let state: serde_json::Value = serde_json::from_str(&state).ok()?;
        state["outputs"]["o"]["value"].as_str().map(str::to_string)
    }

    #[tokio::test]
    async fn the_deploy_stage_applies_the_approved_plan_not_a_fresh_one() {
        let _exe = TERRAFORM_EXE_LOCK.lock().await;
        require_terraform();
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();

        // Plan stage: the reviewer approves input = "approved".
        let (plan_root, plan_dirs) = lay_out(state.path(), "approved");
        let (outputs, saved) = plan_roots(&logger, &dest, &[], plan_root.path(), &plan_dirs, false)
            .await
            .expect("plan");
        assert!(outputs[0].1.contains("will be created"), "{}", outputs[0].1);
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].plan.is_empty());

        // Deploy stage, in a fresh directory whose config has since changed.
        // Before #284 this re-planned and applied "changed" — unreviewed.
        let (apply_root, apply_dirs) = lay_out(state.path(), "changed");
        apply_roots(&logger, &dest, &[], apply_root.path(), &apply_dirs, &saved)
            .await
            .expect("apply the saved plan");

        assert_eq!(state_output(state.path()).as_deref(), Some("approved"));
    }

    #[tokio::test]
    async fn a_stale_plan_fails_the_apply_instead_of_re_planning() {
        let _exe = TERRAFORM_EXE_LOCK.lock().await;
        require_terraform();
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();

        let (plan_root, plan_dirs) = lay_out(state.path(), "approved");
        let (_, saved) = plan_roots(&logger, &dest, &[], plan_root.path(), &plan_dirs, false)
            .await
            .expect("plan");

        // Something else applies between approval and apply.
        let (other_root, other_dirs) = lay_out(state.path(), "someone-else");
        run_command(
            &logger,
            &dest,
            &other_dirs[0],
            &[],
            &["init", "-input=false"],
        )
        .await
        .unwrap();
        run_command(
            &logger,
            &dest,
            &other_dirs[0],
            &[],
            &["apply", "-input=false", "-auto-approve"],
        )
        .await
        .unwrap();
        drop(other_root);

        let (apply_root, apply_dirs) = lay_out(state.path(), "approved");
        let error = apply_roots(&logger, &dest, &[], apply_root.path(), &apply_dirs, &saved)
            .await
            .expect_err("a stale plan must not apply");

        assert!(format!("{error:#}").contains("stale"), "{error:#}");
        assert_eq!(
            state_output(state.path()).as_deref(),
            Some("someone-else"),
            "nothing was applied over the other change"
        );
    }

    #[tokio::test]
    async fn a_destroy_plan_removes_what_it_showed_and_nothing_re_plans_it() {
        let _exe = TERRAFORM_EXE_LOCK.lock().await;
        require_terraform();
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();

        // Something exists.
        let (live_root, live_dirs) = lay_out(state.path(), "live");
        let (_, saved) = plan_roots(&logger, &dest, &[], live_root.path(), &live_dirs, false)
            .await
            .unwrap();
        apply_roots(&logger, &dest, &[], live_root.path(), &live_dirs, &saved)
            .await
            .unwrap();
        assert_eq!(state_output(state.path()).as_deref(), Some("live"));

        // Destroy plan stage.
        let (plan_root, plan_dirs) = lay_out(state.path(), "live");
        let (outputs, saved) = plan_roots(&logger, &dest, &[], plan_root.path(), &plan_dirs, true)
            .await
            .unwrap();
        assert!(
            outputs[0].1.contains("will be destroyed"),
            "{}",
            outputs[0].1
        );

        // Deploy stage applies that saved destroy plan.
        let (apply_root, apply_dirs) = lay_out(state.path(), "live");
        apply_roots(&logger, &dest, &[], apply_root.path(), &apply_dirs, &saved)
            .await
            .unwrap();

        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(state.path().join("terraform.tfstate")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            state["resources"].as_array().map(|r| r.len()),
            Some(0),
            "everything the plan showed is gone: {state}"
        );
    }

    #[tokio::test]
    async fn drift_is_reported_when_the_world_moves_and_not_before() {
        let _exe = TERRAFORM_EXE_LOCK.lock().await;
        require_terraform();
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();
        let managed = tempfile::tempdir().unwrap();
        let file = managed.path().join("managed.txt");

        // Unlike terraform_data, a local_file exists outside terraform and so
        // can drift: the hashicorp/local provider is the one download here.
        let lay_out_file = || {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("dev/^dev/x$/forest/terraform@1");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("main.tf"),
                format!(
                    r#"
terraform {{
  backend "local" {{
    path = "{}/terraform.tfstate"
  }}
}}

resource "local_file" "x" {{
  filename = "{}"
  content  = "managed by forest"
}}
"#,
                    state.path().display(),
                    file.display()
                ),
            )
            .unwrap();
            (root, vec![dir])
        };

        let (root, dirs) = lay_out_file();
        let (_, saved) = plan_roots(&logger, &dest, &[], root.path(), &dirs, false)
            .await
            .unwrap();
        apply_roots(&logger, &dest, &[], root.path(), &dirs, &saved)
            .await
            .unwrap();
        assert!(file.exists());

        let (root, dirs) = lay_out_file();
        let reports = drift_roots(&logger, &dest, &[], root.path(), &dirs)
            .await
            .unwrap();
        assert!(
            !reports[0].drifted,
            "just applied, nothing moved: {}",
            reports[0].output
        );

        // Someone deletes it by hand.
        std::fs::remove_file(&file).unwrap();

        let (root, dirs) = lay_out_file();
        let reports = drift_roots(&logger, &dest, &[], root.path(), &dirs)
            .await
            .unwrap();
        assert!(reports[0].drifted, "{}", reports[0].output);
        assert!(
            reports[0].output.contains("local_file.x"),
            "{}",
            reports[0].output
        );

        // A drift check changes nothing: the state still has the resource,
        // and the file is still gone.
        let state_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(state.path().join("terraform.tfstate")).unwrap(),
        )
        .unwrap();
        assert_eq!(state_json["resources"].as_array().map(|r| r.len()), Some(1));
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn a_root_with_no_saved_plan_is_refused_before_anything_runs() {
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();
        let (root, dirs) = lay_out(state.path(), "x");

        let error = apply_roots(&logger, &dest, &[], root.path(), &dirs, &[])
            .await
            .expect_err("no plan, no apply");

        assert!(format!("{error:#}").contains("saved no plan"), "{error:#}");
        assert!(state_output(state.path()).is_none(), "nothing ran");
    }

    #[test]
    fn every_roots_plan_is_shown_labelled_when_there_are_several() {
        assert_eq!(
            join_plan_outputs(vec![("a".into(), "plan a".into())]),
            "plan a"
        );

        let joined = join_plan_outputs(vec![
            ("a".into(), "plan a".into()),
            ("b".into(), "plan b".into()),
        ]);
        assert!(joined.contains("── a ──\nplan a"), "{joined}");
        assert!(joined.contains("── b ──\nplan b"), "{joined}");
    }

    #[test]
    fn a_work_dir_is_keyed_relative_to_its_release_root() {
        let root = std::path::Path::new("/tmp/abc");
        assert_eq!(
            root_key(root, &root.join("dev/^dev/.*$/forest/terraform@1")),
            "dev/^dev/.*$/forest/terraform@1"
        );
    }
}
