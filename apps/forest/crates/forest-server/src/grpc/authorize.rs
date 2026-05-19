use sqlx::PgPool;
use uuid::Uuid;

use crate::actor::Actor;

/// The minimum required relationship between an actor and an organisation.
#[derive(Debug, Clone, Copy)]
pub enum OrgRole {
    /// Any member of the org (admin or regular member).
    Member,
    /// Must be an admin of the org.
    Admin,
}

/// Successful authorization result, carrying the resolved org_id.
#[allow(dead_code)]
pub struct AuthzContext {
    pub actor: Actor,
    pub organisation_id: Uuid,
}

/// Extract the Actor from tonic request extensions.
pub fn extract_actor(request: &tonic::Request<impl std::any::Any>) -> Result<Actor, tonic::Status> {
    request
        .extensions()
        .get::<Actor>()
        .cloned()
        .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))
}

/// Extract the Actor if present (for endpoints that allow unauthenticated access).
pub fn try_extract_actor(request: &tonic::Request<impl std::any::Any>) -> Option<Actor> {
    request.extensions().get::<Actor>().cloned()
}

/// Verify the actor is authorized for the given organisation (by name).
pub async fn require_org_access(
    db: &PgPool,
    actor: &Actor,
    organisation_name: &str,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    let org = sqlx::query_scalar!(
        "SELECT id FROM organisations WHERE name = $1",
        organisation_name
    )
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!("authz: failed to resolve organisation: {e}");
        tonic::Status::internal("failed to resolve organisation")
    })?
    .ok_or_else(|| tonic::Status::not_found("organisation not found"))?;

    check_org_access(db, actor, org, required_role).await
}

/// Verify the actor is authorized for the given organisation (by UUID).
pub async fn require_org_access_by_id(
    db: &PgPool,
    actor: &Actor,
    organisation_id: Uuid,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    check_org_access(db, actor, organisation_id, required_role).await
}

/// Convenience for handlers with `project: Project { organisation, project }`.
pub async fn require_project_access(
    db: &PgPool,
    actor: &Actor,
    project: &forest_grpc_interface::Project,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    require_org_access(db, actor, &project.organisation, required_role).await
}

async fn check_org_access(
    db: &PgPool,
    actor: &Actor,
    organisation_id: Uuid,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    match actor {
        Actor::ServiceAccount { .. } => {
            // Service accounts bypass org checks (infra-level cross-org access)
            Ok(AuthzContext {
                actor: actor.clone(),
                organisation_id,
            })
        }
        Actor::App {
            organisation_id: app_org_id,
            ..
        } => {
            if *app_org_id != organisation_id {
                return Err(tonic::Status::permission_denied(
                    "app is not scoped to this organisation",
                ));
            }
            Ok(AuthzContext {
                actor: actor.clone(),
                organisation_id,
            })
        }
        Actor::User { user_id } => {
            let member = sqlx::query_scalar!(
                "SELECT role FROM organisation_members WHERE organisation_id = $1 AND user_id = $2",
                organisation_id,
                user_id,
            )
            .fetch_optional(db)
            .await
            .map_err(|e| {
                tracing::error!("authz: failed to check membership: {e}");
                tonic::Status::internal("failed to check membership")
            })?
            .ok_or_else(|| {
                tonic::Status::permission_denied("not a member of this organisation")
            })?;

            if let OrgRole::Admin = required_role
                && member != "admin"
            {
                return Err(tonic::Status::permission_denied("admin access required"));
            }

            Ok(AuthzContext {
                actor: actor.clone(),
                organisation_id,
            })
        }
    }
}
