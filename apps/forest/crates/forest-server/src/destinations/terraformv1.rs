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
        terraform_locks::{
            self, Holder, LockInfo, OwnerState, StaleLockCheck, StaleLockDecision, StateLock,
        },
        terraform_plans::{SavedPlan, TerraformPlanStore},
    },
    services::{artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem},
    temp_dir::TempDirectories,
};

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
    db: PgPool,
    key: CredentialKey,
    stale_lock_after: std::time::Duration,
    replica_gone_after: std::time::Duration,

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

    pub async fn resolve_state_id(
        &self,
        destination: &Destination,
        project_id: &str,
    ) -> anyhow::Result<String> {
        let legacy = Self::legacy_state_id(&destination.environment, project_id);

        sqlx::query!(
            r#"
                INSERT INTO terraform_state_bindings (state_id, organisation, destination)
                VALUES ($1, $2, $3)
                ON CONFLICT (state_id) DO NOTHING
            "#,
            legacy,
            destination.organisation,
            destination.name,
        )
        .execute(&self.db)
        .await
        .context("claim legacy terraform state id")?;

        let binding = sqlx::query!(
            r#"SELECT destination, bound_at FROM terraform_state_bindings WHERE state_id = $1"#,
            legacy,
        )
        .fetch_one(&self.db)
        .await
        .context("read legacy terraform state binding")?;

        if binding.destination == destination.name {
            return Ok(legacy);
        }

        let project_uuid: uuid::Uuid = project_id
            .parse()
            .context("terraform state: project id is not a uuid")?;
        let shared_before = sqlx::query_scalar!(
            r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM release_states rs
                    JOIN destinations d ON d.id = rs.destination_id
                    WHERE rs.project_id = $1
                      AND d.organisation = $2
                      AND d.name = $3
                      AND rs.completed_at IS NOT NULL
                      AND rs.completed_at < $4
                ) AS "shared!"
            "#,
            project_uuid,
            destination.organisation,
            destination.name,
            binding.bound_at,
        )
        .fetch_one(&self.db)
        .await
        .context("check for releases under the shared state id")?;

        if shared_before {
            anyhow::bail!(
                "destination '{}' released this project into environment '{}' while its terraform \
                 state was shared under '{legacy}', which now belongs to destination '{}'. A fresh \
                 state here would plan to re-create what already exists (understory-io/forest#285). \
                 Move this destination's resources into their own state by hand first.",
                destination.name,
                destination.environment,
                binding.destination,
            );
        }

        Ok(Self::destination_state_id(
            &destination.environment,
            project_id,
            &destination.name,
        ))
    }

    pub async fn lookup_state_id(
        &self,
        organisation: &str,
        environment: &str,
        project_id: &str,
        destination: Option<&str>,
    ) -> Result<String, StateLookupError> {
        let legacy = Self::legacy_state_id(environment, project_id);
        let internal = |e: sqlx::Error| StateLookupError::Internal(e.into());

        let owner = sqlx::query_scalar!(
            r#"SELECT destination FROM terraform_state_bindings WHERE state_id = $1"#,
            legacy,
        )
        .fetch_optional(&self.db)
        .await
        .map_err(internal)?;

        let candidates = sqlx::query_scalar!(
            r#"
                SELECT name FROM destinations
                WHERE organisation = $1 AND environment = $2
                  AND type_organisation = 'forest' AND type_name = 'terraform'
                ORDER BY name
            "#,
            organisation,
            environment,
        )
        .fetch_all(&self.db)
        .await
        .map_err(internal)?;

        let destination = match destination {
            Some(asked) if candidates.iter().any(|c| c == asked) => asked.to_string(),
            Some(asked) => {
                return Err(StateLookupError::NotFound(format!(
                    "no forest/terraform@1 destination '{asked}' in environment '{environment}' (have: {})",
                    list_or_none(&candidates)
                )));
            }
            None => match candidates.as_slice() {
                [only] => only.clone(),
                [] if self
                    .exists(&legacy)
                    .await
                    .map_err(StateLookupError::Internal)? =>
                {
                    return Ok(legacy);
                }
                [] => {
                    return Err(StateLookupError::NotFound(format!(
                        "environment '{environment}' has no forest/terraform@1 destination"
                    )));
                }
                _ if owner.is_none()
                    && self
                        .exists(&legacy)
                        .await
                        .map_err(StateLookupError::Internal)? =>
                {
                    return Ok(legacy);
                }
                many => {
                    return Err(StateLookupError::Invalid(format!(
                        "environment '{environment}' has several terraform destinations; name one with --destination ({})",
                        many.join(", ")
                    )));
                }
            },
        };

        Ok(match owner {
            None => legacy,
            Some(owner) if owner == destination => legacy,
            Some(_) => Self::destination_state_id(environment, project_id, &destination),
        })
    }

    async fn exists(&self, state_id: &str) -> anyhow::Result<bool> {
        let exists = sqlx::query_scalar!(
            r#"
                SELECT EXISTS (SELECT 1 FROM terraform_states WHERE state_id = $1)
                    OR EXISTS (SELECT 1 FROM terraform_state_locks WHERE state_id = $1)
                    AS "exists!"
            "#,
            state_id,
        )
        .fetch_one(&self.db)
        .await
        .context("check for terraform state")?;
        Ok(exists)
    }

    pub async fn version(
        &self,
        state_id: &str,
        version: i64,
    ) -> anyhow::Result<Option<(i64, chrono::DateTime<chrono::Utc>, String)>> {
        let row = sqlx::query!(
            r#"
                SELECT id, created_at, content
                FROM terraform_states
                WHERE state_id = $1 AND ($2::BIGINT = 0 OR id = $2)
                ORDER BY id DESC
                LIMIT 1
            "#,
            state_id,
            version,
        )
        .fetch_optional(&self.db)
        .await
        .context("read terraform state version")?;
        Ok(row.map(|r| (r.id, r.created_at, r.content)))
    }

    pub async fn versions(
        &self,
        state_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, chrono::DateTime<chrono::Utc>, i64)>> {
        let rows = sqlx::query!(
            r#"
                SELECT id, created_at, octet_length(content)::BIGINT AS "size!"
                FROM terraform_states
                WHERE state_id = $1
                ORDER BY id DESC
                LIMIT $2
            "#,
            state_id,
            limit,
        )
        .fetch_all(&self.db)
        .await
        .context("list terraform state versions")?;
        Ok(rows
            .into_iter()
            .map(|r| (r.id, r.created_at, r.size))
            .collect())
    }

    pub fn legacy_state_id(environment: &str, project_id: &str) -> String {
        format!("{environment}.{project_id}")
    }

    pub fn destination_state_id(environment: &str, project_id: &str, destination: &str) -> String {
        use sha2::Digest as _;

        let mut slug = String::with_capacity(destination.len());
        for c in destination.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug: String = slug.trim_matches('-').chars().take(40).collect();
        let hash = hex::encode(sha2::Sha256::digest(destination.as_bytes()));

        format!("{environment}.{project_id}.{slug}-{}", &hash[..12])
    }

    pub fn in_process_credential(
        &self,
        state_id: &str,
        destination: &Destination,
        release_id: uuid::Uuid,
    ) -> String {
        self.key.issue(
            state_id,
            Scope::ReadWrite,
            &terraform_locks::in_process_principal(
                release_id,
                crate::replicas::replica_name(),
                destination,
            ),
            terraform_credentials::RUNNER_TTL,
        )
    }

    pub fn runner_credential(
        &self,
        state_id: &str,
        release_id: uuid::Uuid,
        runner_id: &str,
    ) -> String {
        self.key.issue(
            state_id,
            Scope::ReadWrite,
            &terraform_locks::runner_principal(release_id, runner_id),
            terraform_credentials::RUNNER_TTL,
        )
    }

    pub fn operator_credential(
        &self,
        state_id: &str,
        scope: Scope,
        principal: &str,
        ttl: std::time::Duration,
    ) -> (String, u64) {
        self.key.issue_with_expiry(
            state_id,
            scope,
            principal,
            ttl.min(terraform_credentials::MAX_OPERATOR_TTL),
        )
    }

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

    pub async fn current_lock(&self, state_id: &str) -> anyhow::Result<Option<StateLock>> {
        let row = sqlx::query!(
            r#"
                SELECT state_id, lock_id, operation, who, info, tf_version, tf_created, path,
                       principal, release_id, replica, acquired_at
                FROM terraform_state_locks
                WHERE state_id = $1
            "#,
            state_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("read terraform state lock")?;

        Ok(row.map(|r| StateLock {
            state_id: r.state_id,
            lock_id: r.lock_id,
            operation: r.operation,
            who: r.who,
            info: r.info,
            tf_version: r.tf_version,
            tf_created: r.tf_created,
            path: r.path,
            principal: r.principal,
            release_id: r.release_id,
            replica: r.replica,
            acquired_at: r.acquired_at,
        }))
    }

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

    pub async fn release_locks_held_by(
        &self,
        release_id: uuid::Uuid,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let rows = sqlx::query!(
            r#"
                DELETE FROM terraform_state_locks
                WHERE release_id = $1
                  AND principal LIKE 'release:%'
                  AND replica = $2
                RETURNING state_id, lock_id
            "#,
            release_id,
            crate::replicas::replica_name(),
        )
        .fetch_all(&self.db)
        .await
        .context("release terraform state locks held by release")?;

        let released: Vec<(String, String)> =
            rows.into_iter().map(|r| (r.state_id, r.lock_id)).collect();
        for (state_id, lock_id) in &released {
            tracing::warn!(
                %release_id,
                state_id,
                lock_id,
                "released the terraform state lock of an in-process release whose terraform has exited"
            );
        }
        Ok(released)
    }

    pub async fn check_stale_lock(
        &self,
        state_id: &str,
        release: &ReleaseItem,
    ) -> anyhow::Result<StaleLockCheck> {
        let Some(lock) = self.current_lock(state_id).await? else {
            return Ok(StaleLockCheck::Unlocked);
        };
        if lock.release_id == Some(release.id) {
            return Ok(StaleLockCheck::HeldByThisRelease);
        }

        let owner = match lock.holder_kind() {
            Holder::InProcessRelease {
                release_id,
                replica,
            } => {
                let status = sqlx::query_scalar!(
                    r#"SELECT status FROM release_states WHERE release_id = $1"#,
                    release_id,
                )
                .fetch_optional(&self.db)
                .await
                .context("read the status of the release holding the lock")?;
                let replica_liveness = match replica.as_deref() {
                    Some(name) => {
                        crate::replicas::liveness(&self.db, name, self.replica_gone_after).await?
                    }
                    None => crate::replicas::Liveness::Gone,
                };
                OwnerState::InProcessRelease {
                    release_id,
                    status,
                    replica,
                    replica_liveness,
                }
            }
            Holder::RunnerRelease { .. } => OwnerState::RunnerRelease,
            Holder::Operator { .. } => OwnerState::Operator,
            Holder::Legacy => {
                let other_release_active = sqlx::query_scalar!(
                    r#"
                        SELECT EXISTS (
                            SELECT 1 FROM release_states
                            WHERE destination_id = $1
                              AND release_id <> $2
                              AND status IN ('ASSIGNED', 'RUNNING')
                        ) AS "exists!"
                    "#,
                    release.destination_id,
                    release.id,
                )
                .fetch_one(&self.db)
                .await
                .context("check for other running releases on the destination")?;
                OwnerState::Legacy {
                    other_release_active,
                }
            }
        };

        match terraform_locks::decide(&lock, &owner, chrono::Utc::now(), self.stale_lock_after) {
            StaleLockDecision::Keep(reason) => Ok(StaleLockCheck::Kept { lock, reason }),
            StaleLockDecision::Release(reason) => {
                let removed = sqlx::query!(
                    r#"
                        DELETE FROM terraform_state_locks
                        WHERE state_id = $1 AND lock_id = $2
                    "#,
                    state_id,
                    lock.lock_id,
                )
                .execute(&self.db)
                .await
                .context("release stale terraform state lock")?
                .rows_affected();
                if removed == 0 {
                    return Ok(StaleLockCheck::Kept {
                        lock,
                        reason: "the lock changed hands while it was being checked".into(),
                    });
                }
                tracing::warn!(
                    state_id,
                    lock_id = %lock.lock_id,
                    holder = %lock.holder(),
                    release_id = %release.id,
                    reason,
                    "released a stale terraform state lock before running terraform"
                );
                Ok(StaleLockCheck::Released { lock, reason })
            }
        }
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

    async fn attempt_lock(
        &self,
        project_id: String,
        info: &LockInfo,
        principal: &str,
    ) -> anyhow::Result<LockState> {
        let lock_id = info.id.as_str();
        let holder = Holder::parse(Some(principal));
        let release_id = holder.release_id();
        let replica = holder.replica().map(str::to_string);
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
                        INSERT INTO terraform_state_locks (
                            state_id, lock_id, operation, who, info, tf_version, tf_created,
                            path, principal, release_id, replica
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    "#,
                    project_id,
                    lock_id,
                    info.operation,
                    info.who,
                    info.info,
                    info.version,
                    info.created,
                    info.path,
                    principal,
                    release_id,
                    replica,
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

#[derive(Debug)]
pub enum StateLookupError {
    NotFound(String),
    Invalid(String),
    Internal(anyhow::Error),
}

fn list_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
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
        TerraformStateStore {
            db: self.db.clone(),
            key: CredentialKey::from_secrets(
                self.config.terraform_state_signing_key.as_deref(),
                &self.config.access_token_secret_key,
            ),
            stale_lock_after: self.config.terraform_stale_lock_after,
            replica_gone_after: self.config.replica_gone_after,

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
        let verified = Self::authorise(&state, &state_id, &headers, "lock", true)?;
        let info: LockInfo = serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest)?;
        if info.id.is_empty() {
            return Err(ApiError::BadRequest);
        }
        tracing::info!(
            lock_id = info.id,
            state_id,
            operation = info.operation,
            who = info.who,
            principal = %verified.principal,
            "locking state"
        );

        let store = state.terraform_state_store();
        let lock = match store
            .attempt_lock(state_id.clone(), &info, &verified.principal)
            .await
        {
            Ok(lock) => lock,
            Err(e) => {
                tracing::warn!("failed to lock state: {e:#}");
                return Err(ApiError::InternalServerError);
            }
        };

        match lock {
            LockState::Held => Ok(StatusCode::OK.into_response()),
            LockState::Wait => {
                let holder = store.current_lock(&state_id).await.ok().flatten();
                let body = match holder {
                    Some(holder) => {
                        tracing::info!(
                            state_id,
                            requested_by = %verified.principal,
                            "terraform state is locked: {}",
                            holder.describe()
                        );
                        holder.conflict_info()
                    }
                    None => LockInfo {
                        created: chrono::Utc::now().to_rfc3339(),
                        ..LockInfo::default()
                    },
                };
                Ok((StatusCode::CONFLICT, Json(body)).into_response())
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
    fn parse(body: &[u8]) -> Result<Self, ApiError> {
        serde_json::from_slice(body).map_err(|_| ApiError::BadRequest)
    }
}

struct StatePath {}

pub enum ApiError {
    BadRequest,
    NotFound,
    InternalServerError,
    Unauthorized,
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

        dirs.sort();
        dirs.dedup();

        return Ok(dirs);
    }

    legacy_work_dirs(temp_dir, destination).await
}

fn ensure_one_root(dirs: &[std::path::PathBuf], destination: &Destination) -> anyhow::Result<()> {
    match dirs {
        [] => anyhow::bail!("failed to find a destination match for submitted release"),
        [_] => Ok(()),
        many => anyhow::bail!(
            "{} terraform roots selected destination '{}' ({}); they would share one state and \
             destroy each other's resources (understory-io/forest#285). Make one component usage \
             select this destination.",
            many.len(),
            destination.name,
            many.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}

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

struct Staged {
    _temp: crate::temp_dir::GuardedTempDirectory,
    root: std::path::PathBuf,
    dirs: Vec<std::path::PathBuf>,
}

fn root_key(root: &std::path::Path, dir: &std::path::Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .to_string_lossy()
        .into_owned()
}

pub fn report_stale_lock_check(
    logger: &DestinationLogger,
    state_id: &str,
    check: anyhow::Result<StaleLockCheck>,
) {
    match check {
        Ok(StaleLockCheck::Unlocked) | Ok(StaleLockCheck::HeldByThisRelease) => {}
        Ok(StaleLockCheck::Kept { lock, reason }) => {
            tracing::info!(
                state_id,
                lock_id = %lock.lock_id,
                holder = %lock.holder(),
                "terraform state is locked, leaving the lock: {reason}"
            );
            logger.log_stderr(&format!(
                "forest: terraform state {state_id} is locked ({}); leaving it: {reason}",
                lock.describe()
            ));
        }
        Ok(StaleLockCheck::Released { lock, reason }) => {
            logger.log_stderr(&format!(
                "forest: released stale terraform state lock on {state_id} ({}): {reason}",
                lock.describe()
            ));
        }
        Err(e) => {
            tracing::warn!(state_id, "could not check the terraform state lock: {e:#}");
        }
    }
}

const PLAN_FILE: &str = "forest.tfplan";
const LOCK_FILE: &str = ".terraform.lock.hcl";

impl TerraformV1Destination {
    async fn backend_env(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Vec<(&'static str, String)>> {
        let state_id = self
            .tf_state
            .resolve_state_id(destination, &release.project_id.to_string())
            .await?;
        Ok(self.env_for(&state_id, destination, release))
    }

    async fn backend_env_clearing_stale_lock(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<Vec<(&'static str, String)>> {
        let state_id = self
            .tf_state
            .resolve_state_id(destination, &release.project_id.to_string())
            .await?;
        report_stale_lock_check(
            logger,
            &state_id,
            self.tf_state.check_stale_lock(&state_id, release).await,
        );
        Ok(self.env_for(&state_id, destination, release))
    }

    fn env_for(
        &self,
        state_id: &str,
        destination: &Destination,
        release: &ReleaseItem,
    ) -> Vec<(&'static str, String)> {
        let password = self
            .tf_state
            .in_process_credential(state_id, destination, release.id);
        let (base, lock, unlock) = self.tf_state.addresses(state_id);
        vec![
            ("TF_HTTP_LOCK_ADDRESS", lock),
            ("TF_HTTP_UNLOCK_ADDRESS", unlock),
            ("TF_HTTP_ADDRESS", base),
            ("TF_HTTP_PASSWORD", password),
        ]
    }

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
        ensure_one_root(&dirs, destination)?;

        Ok(Staged {
            _temp: temp,
            root,
            dirs,
        })
    }

    async fn plan_and_save(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<String> {
        let destroy = self.plans.is_destroy_plan(release.id).await?;
        let env = self
            .backend_env_clearing_stale_lock(logger, release, destination)
            .await?;
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
            return Ok(format!(
                "⚠ DESTROY PLAN — approving this removes everything shown below, and the deploy \
                 stage applies it as-is.\n\n{output}"
            ));
        }
        Ok(output)
    }

    async fn apply_saved(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        saved: Vec<SavedPlan>,
    ) -> anyhow::Result<()> {
        let env = self
            .backend_env_clearing_stale_lock(logger, release, destination)
            .await?;
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

    async fn run_unplanned(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let env = self
            .backend_env_clearing_stale_lock(logger, release, destination)
            .await?;
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

async fn plan_roots(
    logger: &DestinationLogger,
    destination: &Destination,
    env: &[(&str, String)],
    root: &std::path::Path,
    dirs: &[std::path::PathBuf],
    destroy: bool,
) -> anyhow::Result<(Vec<(String, String)>, Vec<SavedPlan>)> {
    let mut plan_args = vec![
        "plan",
        "-input=false",
        "-detailed-exitcode",
        "-out",
        PLAN_FILE,
    ];
    if destroy {
        plan_args.push("-destroy");
    }

    let mut outputs = Vec::new();
    let mut saved = Vec::new();
    for dir in dirs {
        run_command(logger, destination, dir, env, &["init", "-input=false"])
            .await
            .context("terraform init")?;

        let (code, output) = run_command_status(logger, destination, dir, env, &plan_args)
            .await
            .context("terraform plan")?;
        let has_changes = match code {
            0 => false,
            2 => true,
            other => anyhow::bail!("terraform plan failed: {other}"),
        };

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
            has_changes,
        });
    }

    Ok((outputs, saved))
}

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

async fn run_command_status(
    logger: &DestinationLogger,
    destination: &Destination,
    path: &std::path::Path,
    tf_envs: &[(&str, String)],
    args: &[&str],
) -> anyhow::Result<(i32, String)> {
    tracing::debug!(path =% path.display(), "running terraform {} (capture)", args.join(" "));

    crate::release_drain::ensure_not_interrupted()?;

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

    let exit = crate::release_drain::wait_interruptible(&mut proc)
        .await
        .context("terraform failed")?;
    if let Some(task) = stdout_task {
        let _ = task.await;
    }

    let output = captured.lock().await.clone();
    Ok((exit.code().unwrap_or(-1), output))
}

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

    crate::release_drain::ensure_not_interrupted()?;

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

    let exit = crate::release_drain::wait_interruptible(&mut proc)
        .await
        .context("terraform failed")?;
    if !exit.success() {
        anyhow::bail!("terraform failed: {}", exit.code().unwrap_or(-1));
    }

    tracing::debug!("terraform command success");

    Ok(())
}

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

    if let Some(cache) = plugin_cache_dir().await {
        cmd.env("TF_PLUGIN_CACHE_DIR", cache)
            .env("TF_PLUGIN_CACHE_MAY_BREAK_DEPENDENCY_LOCK_FILE", "true");
    }

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

    if let Some(aws_config) = write_aws_profile(path, destination).await {
        cmd.env("AWS_CONFIG_FILE", &aws_config)
            .env("AWS_PROFILE", "forest")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env_remove("AWS_SESSION_TOKEN");
    }

    if let Some(n) = parallelism(destination) {
        let arg = format!("-parallelism={n}");
        cmd.env("TF_CLI_ARGS_plan", &arg)
            .env("TF_CLI_ARGS_apply", &arg);
    }

    for (k, v) in &destination.metadata {
        cmd.env(format!("TF_VAR_{}", k), v);
    }

    cmd
}

fn parallelism(destination: &Destination) -> Option<u32> {
    destination
        .metadata
        .get("tf_parallelism")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
}

async fn plugin_cache_dir() -> Option<std::path::PathBuf> {
    let dir = match std::env::var("FOREST_TERRAFORM_PLUGIN_CACHE_DIR") {
        Ok(dir) if dir.is_empty() => return None,
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => std::env::temp_dir()
            .join("forest")
            .join("terraform-plugin-cache"),
    };
    match tokio::fs::create_dir_all(&dir).await {
        Ok(()) => Some(dir),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), "terraform plugin cache unavailable, downloading providers per run: {e}");
            None
        }
    }
}

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
        let env = self.backend_env(deployed, destination).await?;
        let staged = self.stage(deployed, destination).await?;
        let reports = drift_roots(logger, destination, &env, &staged.root, &staged.dirs).await?;
        Ok(Some(reports))
    }
}

enum Mode {
    Prepare,
    Apply,
}

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

    let mut lines: Vec<String> = Vec::new();

    if !access_key.is_empty() && !secret_key.is_empty() {
        lines.push("[profile forest-source]".into());
        lines.push(format!("aws_access_key_id = {access_key}"));
        lines.push(format!("aws_secret_access_key = {secret_key}"));
        lines.push(String::new());
        lines.push("[profile forest]".into());
        lines.push(format!("role_arn = {role_arn}"));
        lines.push("source_profile = forest-source".into());
    } else {
        lines.push("[profile forest]".into());
        lines.push(format!("role_arn = {role_arn}"));
        lines.push("credential_source = EcsContainer".into());
    }

    lines.push(format!("role_session_name = {session_name}"));
    lines.push(format!("region = {region}"));

    let config = format!("{}\n", lines.join("\n"));

    let config_path = path.join(".aws-config");
    if let Err(e) = tokio::fs::write(&config_path, config).await {
        tracing::error!(
            error = %e,
            path = %config_path.display(),
            "could not write the AWS profile for the assumed role"
        );
        return None;
    }

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

    #[test]
    fn parallelism_comes_from_the_destination_and_ignores_nonsense() {
        assert_eq!(
            parallelism(&destination_with(&[("tf_parallelism", "30")])),
            Some(30)
        );
        assert_eq!(
            parallelism(&destination_with(&[("tf_parallelism", " 5 ")])),
            Some(5)
        );
        assert_eq!(
            parallelism(&destination_with(&[("tf_parallelism", "0")])),
            None
        );
        assert_eq!(
            parallelism(&destination_with(&[("tf_parallelism", "lots")])),
            None
        );
        assert_eq!(parallelism(&destination_with(&[])), None);
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

        assert!(config.contains("[profile forest-source]"));
        assert!(config.contains("source_profile = forest-source"));
        assert!(config.contains("aws_access_key_id = AKIAEXAMPLE"));
        assert!(!config.contains("credential_source"));
    }

    #[tokio::test]
    async fn the_session_name_is_bounded_and_sts_safe() {
        let dir = tempfile::tempdir().unwrap();
        let mut dest = destination_with(&[("aws_role_arn", "arn:aws:iam::1:role/r")]);
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
        let _exe = TERRAFORM_EXE_LOCK.blocking_lock();
        let prev = std::env::var("TERRAFORM_EXE").ok();

        unsafe {
            std::env::set_var("TERRAFORM_EXE", "my-custom-tf");
        }
        assert_eq!(resolve_terraform_exe(), "my-custom-tf");

        unsafe {
            std::env::set_var("TERRAFORM_EXE", "");
        }
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

    #[test]
    fn a_destination_state_id_is_one_url_segment_and_unique_per_name() {
        let a = TerraformStateStore::destination_state_id("dev", "p1", "dev/eu-west-1/core");
        let b = TerraformStateStore::destination_state_id("dev", "p1", "dev-eu-west-1-core");

        assert!(!a.contains('/'), "{a}");
        assert!(a.starts_with("dev.p1.dev-eu-west-1-core-"), "{a}");
        assert_ne!(a, b);
        assert_eq!(
            a,
            TerraformStateStore::destination_state_id("dev", "p1", "dev/eu-west-1/core")
        );
        assert_ne!(a, TerraformStateStore::legacy_state_id("dev", "p1"));
    }

    #[test]
    fn one_destination_takes_exactly_one_terraform_root() {
        let dest = destination("dev/eu-west-1/core", "dev", "terraform");
        let root = std::path::PathBuf::from("/w/dev/a/forest/terraform@1");
        let other = std::path::PathBuf::from("/w/dev/b/forest/terraform@1");

        assert!(ensure_one_root(std::slice::from_ref(&root), &dest).is_ok());
        assert!(ensure_one_root(&[], &dest).is_err());

        let err = ensure_one_root(&[root, other], &dest)
            .unwrap_err()
            .to_string();
        assert!(err.contains("share one state"), "{err}");
    }

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

        let (plan_root, plan_dirs) = lay_out(state.path(), "approved");
        let (outputs, saved) = plan_roots(&logger, &dest, &[], plan_root.path(), &plan_dirs, false)
            .await
            .expect("plan");
        assert!(outputs[0].1.contains("will be created"), "{}", outputs[0].1);
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].plan.is_empty());
        assert!(saved[0].has_changes, "creating a resource is a change");

        let (apply_root, apply_dirs) = lay_out(state.path(), "changed");
        apply_roots(&logger, &dest, &[], apply_root.path(), &apply_dirs, &saved)
            .await
            .expect("apply the saved plan");

        assert_eq!(state_output(state.path()).as_deref(), Some("approved"));
    }

    #[tokio::test]
    async fn a_plan_against_state_that_already_matches_is_saved_as_changing_nothing() {
        let _exe = TERRAFORM_EXE_LOCK.lock().await;
        require_terraform();
        let logger = test_logger().await;
        let dest = destination_with(&[]);
        let state = tempfile::tempdir().unwrap();

        let (first_root, first_dirs) = lay_out(state.path(), "same");
        let (_, first) = plan_roots(&logger, &dest, &[], first_root.path(), &first_dirs, false)
            .await
            .expect("plan");
        apply_roots(&logger, &dest, &[], first_root.path(), &first_dirs, &first)
            .await
            .expect("apply");

        let (again_root, again_dirs) = lay_out(state.path(), "same");
        let (outputs, again) =
            plan_roots(&logger, &dest, &[], again_root.path(), &again_dirs, false)
                .await
                .expect("plan again");

        assert!(outputs[0].1.contains("No changes"), "{}", outputs[0].1);
        assert!(!again[0].has_changes);
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

        let (live_root, live_dirs) = lay_out(state.path(), "live");
        let (_, saved) = plan_roots(&logger, &dest, &[], live_root.path(), &live_dirs, false)
            .await
            .unwrap();
        apply_roots(&logger, &dest, &[], live_root.path(), &live_dirs, &saved)
            .await
            .unwrap();
        assert_eq!(state_output(state.path()).as_deref(), Some("live"));

        let (plan_root, plan_dirs) = lay_out(state.path(), "live");
        let (outputs, saved) = plan_roots(&logger, &dest, &[], plan_root.path(), &plan_dirs, true)
            .await
            .unwrap();
        assert!(
            outputs[0].1.contains("will be destroyed"),
            "{}",
            outputs[0].1
        );

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
