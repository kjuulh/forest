//! Operator access to terraform@1 state (understory-io/forest#282).
//!
//! Every call requires **org Admin** — reads included, because a pulled state
//! carries the secrets terraform wrote into it. Kasper's call, for now; the
//! bar can come down per call later, but not silently. Every call emits an
//! audit event and a log line naming the actor and the state id.

use std::time::Duration;

use forest_grpc_interface::{terraform_state_service_server::TerraformStateService, *};
use tonic::Response;

use crate::{
    actor::Actor,
    destinations::{
        terraform_credentials::Scope,
        terraformv1::{StateLookupError, TerraformStateStoreState},
    },
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::event_bus::{EventBusState, EventPayload},
    state::State,
};

pub struct TerraformStateServer {
    pub state: State,
}

const DEFAULT_TTL: Duration = Duration::from_secs(60 * 60);

fn principal(actor: &Actor) -> String {
    format!("{}:{}", actor.actor_type(), actor.actor_id())
}

impl TerraformStateServer {
    /// Resolve a reference to a state id. Callers authorise first.
    async fn resolve(&self, reference: &TerraformStateRef) -> Result<String, tonic::Status> {
        if reference.environment.is_empty() || reference.project.is_empty() {
            return Err(tonic::Status::invalid_argument(
                "environment and project are required",
            ));
        }

        let project_id = sqlx::query_scalar!(
            r#"SELECT id FROM projects WHERE organisation = $1 AND project = $2"#,
            reference.organisation,
            reference.project,
        )
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| tonic::Status::internal(format!("look up project: {e}")))?
        .ok_or_else(|| {
            tonic::Status::not_found(format!(
                "project {}/{} not found",
                reference.organisation, reference.project
            ))
        })?;

        let destination =
            (!reference.destination.is_empty()).then_some(reference.destination.as_str());

        self.state
            .terraform_state_store()
            .lookup_state_id(
                &reference.organisation,
                &reference.environment,
                &project_id.to_string(),
                destination,
            )
            .await
            .map_err(|e| match e {
                StateLookupError::NotFound(m) => tonic::Status::not_found(m),
                StateLookupError::Invalid(m) => tonic::Status::failed_precondition(m),
                StateLookupError::Internal(e) => tonic::Status::internal(format!("{e:#}")),
            })
    }

    async fn audit(
        &self,
        actor: &Actor,
        reference: &TerraformStateRef,
        state_id: &str,
        action: &'static str,
        detail: &[(&str, String)],
    ) {
        let mut metadata: std::collections::BTreeMap<String, String> = detail
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        metadata.insert("state_id".into(), state_id.to_string());
        metadata.insert("environment".into(), reference.environment.clone());
        metadata.insert("actor".into(), principal(actor));

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: reference.organisation.clone(),
                project: reference.project.clone(),
                resource_type: "terraform_state",
                action,
                resource_id: state_id.to_string(),
                metadata,
            })
            .await;

        tracing::info!(
            organisation = %reference.organisation,
            project = %reference.project,
            environment = %reference.environment,
            state_id,
            actor = %principal(actor),
            action,
            ?detail,
            "terraform state accessed"
        );
    }
}

fn reference(state: Option<TerraformStateRef>) -> Result<TerraformStateRef, tonic::Status> {
    let reference = state.ok_or_else(|| tonic::Status::invalid_argument("state is required"))?;
    if reference.organisation.is_empty() {
        return Err(tonic::Status::invalid_argument("organisation is required"));
    }
    Ok(reference)
}

#[async_trait::async_trait]
impl TerraformStateService for TerraformStateServer {
    async fn issue_terraform_state_credential(
        &self,
        request: tonic::Request<IssueTerraformStateCredentialRequest>,
    ) -> std::result::Result<tonic::Response<IssueTerraformStateCredentialResponse>, tonic::Status>
    {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let reference = reference(req.state)?;
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &reference.organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let state_id = self.resolve(&reference).await?;
        let scope = if req.write {
            Scope::ReadWrite
        } else {
            Scope::ReadOnly
        };
        let ttl = match req.ttl_seconds {
            0 => DEFAULT_TTL,
            s => Duration::from_secs(s),
        };

        let store = self.state.terraform_state_store();
        let (password, expires_at) =
            store.operator_credential(&state_id, scope, &principal(&actor), ttl);
        let expires_at_unix = i64::try_from(expires_at).unwrap_or(i64::MAX);
        let (address, lock_address, unlock_address) = store.addresses(&state_id);

        self.audit(
            &actor,
            &reference,
            &state_id,
            "credential_issued",
            &[
                ("scope", if req.write { "rw" } else { "ro" }.into()),
                ("expires_at", expires_at_unix.to_string()),
            ],
        )
        .await;

        Ok(Response::new(IssueTerraformStateCredentialResponse {
            state_id,
            address,
            lock_address,
            unlock_address,
            username: "forest-terraform-v1".into(),
            password,
            expires_at_unix,
            write: req.write,
        }))
    }

    async fn get_terraform_state(
        &self,
        request: tonic::Request<GetTerraformStateRequest>,
    ) -> std::result::Result<tonic::Response<GetTerraformStateResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let reference = reference(req.state)?;
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &reference.organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let state_id = self.resolve(&reference).await?;
        let (version, created_at, content) = self
            .state
            .terraform_state_store()
            .version(&state_id, req.version)
            .await
            .to_internal_error()?
            .ok_or_else(|| {
                tonic::Status::not_found(match req.version {
                    0 => format!("no terraform state stored for {state_id} yet"),
                    v => format!("{state_id} has no version {v}"),
                })
            })?;

        self.audit(
            &actor,
            &reference,
            &state_id,
            "state_read",
            &[("version", version.to_string())],
        )
        .await;

        Ok(Response::new(GetTerraformStateResponse {
            state_id,
            version,
            created_at: created_at.to_rfc3339(),
            content,
        }))
    }

    async fn list_terraform_state_versions(
        &self,
        request: tonic::Request<ListTerraformStateVersionsRequest>,
    ) -> std::result::Result<tonic::Response<ListTerraformStateVersionsResponse>, tonic::Status>
    {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let reference = reference(req.state)?;
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &reference.organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let state_id = self.resolve(&reference).await?;
        let store = self.state.terraform_state_store();
        let limit = match req.limit {
            0 => 20,
            n => i64::from(n.min(500)),
        };
        let versions = store
            .versions(&state_id, limit)
            .await
            .to_internal_error()?
            .into_iter()
            .map(|(version, created_at, size)| TerraformStateVersion {
                version,
                created_at: created_at.to_rfc3339(),
                size_bytes: size.max(0) as u64,
            })
            .collect();
        let lock_id = store
            .lock_holder(&state_id)
            .await
            .to_internal_error()?
            .unwrap_or_default();

        Ok(Response::new(ListTerraformStateVersionsResponse {
            state_id,
            versions,
            lock_id,
        }))
    }

    async fn force_unlock_terraform_state(
        &self,
        request: tonic::Request<ForceUnlockTerraformStateRequest>,
    ) -> std::result::Result<tonic::Response<ForceUnlockTerraformStateResponse>, tonic::Status>
    {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let reference = reference(req.state)?;
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &reference.organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let state_id = self.resolve(&reference).await?;
        let removed = self
            .state
            .terraform_state_store()
            .force_unlock(&state_id)
            .await
            .to_internal_error()?
            .unwrap_or_default();

        self.audit(
            &actor,
            &reference,
            &state_id,
            "force_unlocked",
            &[("lock_id", removed.clone())],
        )
        .await;

        Ok(Response::new(ForceUnlockTerraformStateResponse {
            state_id,
            removed_lock_id: removed,
        }))
    }
}
