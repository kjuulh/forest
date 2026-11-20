use sqlx::PgPool;
use uuid::Uuid;

use crate::actor::Actor;

#[derive(Debug, Clone, Copy)]
pub enum OrgRole {
    Member,
    Admin,
}

#[allow(dead_code)]
pub struct AuthzContext {
    pub actor: Actor,
    pub organisation_id: Uuid,
}

pub fn extract_actor(request: &tonic::Request<impl std::any::Any>) -> Result<Actor, tonic::Status> {
    request
        .extensions()
        .get::<Actor>()
        .cloned()
        .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))
}

pub fn try_extract_actor(request: &tonic::Request<impl std::any::Any>) -> Option<Actor> {
    request.extensions().get::<Actor>().cloned()
}

#[derive(Debug)]
#[must_use = "extract an UnauthenticatedActor only to use it — call .require_authenticated()"]
pub struct UnauthenticatedActor(Option<Actor>);

impl UnauthenticatedActor {
    pub fn require_authenticated(self) -> Result<AuthenticatedActor, tonic::Status> {
        self.0
            .map(AuthenticatedActor)
            .ok_or_else(|| tonic::Status::unauthenticated("authentication required"))
    }
}

#[derive(Debug)]
#[must_use = "extract an AuthenticatedActor only to use it — call a require_* method or into_actor"]
pub struct AuthenticatedActor(Actor);

impl AuthenticatedActor {
    pub fn require_user_self_or_service_account(
        self,
        target_user_id: Uuid,
    ) -> Result<Actor, tonic::Status> {
        match self.0 {
            Actor::User { user_id } if user_id == target_user_id => Ok(self.0),
            Actor::ServiceAccount { .. } => Ok(self.0),
            _ => Err(tonic::Status::permission_denied(
                "operation restricted to the target user",
            )),
        }
    }

    pub fn require_service_account(self) -> Result<Actor, tonic::Status> {
        match self.0 {
            Actor::ServiceAccount { .. } => Ok(self.0),
            _ => Err(tonic::Status::permission_denied(
                "this operation requires service account authentication",
            )),
        }
    }

    pub fn require_user_self(self, target_user_id: Uuid) -> Result<Actor, tonic::Status> {
        match self.0 {
            Actor::User { user_id } if user_id == target_user_id => Ok(self.0),
            _ => Err(tonic::Status::permission_denied(
                "operation restricted to the target user",
            )),
        }
    }

    pub fn into_actor(self) -> Actor {
        self.0
    }
}

pub fn unauthenticated_actor<T>(request: &tonic::Request<T>) -> UnauthenticatedActor {
    UnauthenticatedActor(request.extensions().get::<Actor>().cloned())
}

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

pub async fn require_org_access_by_id(
    db: &PgPool,
    actor: &Actor,
    organisation_id: Uuid,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    check_org_access(db, actor, organisation_id, required_role).await
}

/// Secrets and approval gates are operator actions. An org-scoped app token
/// passes ordinary org checks without a user role; it must not inherit admin
/// authority to reveal provider credentials or approve production plans.
pub async fn require_operator_access(
    db: &PgPool,
    actor: &Actor,
    organisation_name: &str,
) -> Result<AuthzContext, tonic::Status> {
    if matches!(actor, Actor::App { .. }) {
        return Err(tonic::Status::permission_denied(
            "app tokens cannot reveal credentials or approve plans",
        ));
    }
    require_org_access(db, actor, organisation_name, OrgRole::Admin).await
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

/// Read access to a published component, as a consumer resolving a dependency.
///
/// Members of the owning organisation may read anything it publishes. Anyone
/// authenticated may read a component when its project or its organisation is
/// `public`; every other component stays refused to non-members. Writes never
/// come through here: they keep requiring membership.
pub async fn require_component_read(
    db: &PgPool,
    actor: &Actor,
    organisation: &str,
    name: &str,
) -> Result<(), tonic::Status> {
    let denied = match require_org_access(db, actor, organisation, OrgRole::Member).await {
        Ok(_) => return Ok(()),
        Err(status) if status.code() == tonic::Code::PermissionDenied => status,
        Err(status) => return Err(status),
    };

    if component_is_public(db, organisation, name).await? {
        Ok(())
    } else {
        Err(denied)
    }
}

/// Read access to an organisation's published component listing: its members,
/// or anyone authenticated when the organisation is `public`.
pub async fn require_org_read(
    db: &PgPool,
    actor: &Actor,
    organisation: &str,
) -> Result<(), tonic::Status> {
    let denied = match require_org_access(db, actor, organisation, OrgRole::Member).await {
        Ok(_) => return Ok(()),
        Err(status) if status.code() == tonic::Code::PermissionDenied => status,
        Err(status) => return Err(status),
    };

    if org_is_public(db, organisation).await? {
        Ok(())
    } else {
        Err(denied)
    }
}

/// Who may change what non-members can read: a human admin of the organisation
/// or a service account. App tokens never may, whatever their scope: any
/// member can mint one, so honouring them would let a plain member, or a
/// leaked CI publish token, open the organisation's components to everyone.
pub async fn require_visibility_admin(
    db: &PgPool,
    actor: &Actor,
    organisation: &str,
) -> Result<(), tonic::Status> {
    if let Actor::App { .. } = actor {
        return Err(tonic::Status::permission_denied(
            "changing visibility needs an organisation admin, not an app token",
        ));
    }
    require_org_access(db, actor, organisation, OrgRole::Admin).await?;
    Ok(())
}

pub async fn can_change_visibility(
    db: &PgPool,
    actor: &Actor,
    organisation: &str,
) -> Result<bool, tonic::Status> {
    match require_visibility_admin(db, actor, organisation).await {
        Ok(()) => Ok(true),
        Err(status) if status.code() == tonic::Code::PermissionDenied => Ok(false),
        Err(status) => Err(status),
    }
}

pub fn audit_visibility_change(actor: &Actor, organisation: &str, target: &str, visibility: &str) {
    tracing::info!(
        actor = ?actor,
        organisation,
        target,
        visibility,
        "visibility changed"
    );
}

pub async fn component_is_public(
    db: &PgPool,
    organisation: &str,
    name: &str,
) -> Result<bool, tonic::Status> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM projects p
            WHERE p.organisation = $1 AND p.project = $2 AND p.visibility = 'public'
        ) OR EXISTS(
            SELECT 1 FROM organisations o WHERE o.name = $1 AND o.visibility = 'public'
        )",
    )
    .bind(organisation)
    .bind(name)
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!("authz: failed to check component visibility: {e}");
        tonic::Status::internal("failed to check component visibility")
    })
}

async fn org_is_public(db: &PgPool, organisation: &str) -> Result<bool, tonic::Status> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM organisations o WHERE o.name = $1 AND o.visibility = 'public')",
    )
    .bind(organisation)
    .fetch_one(db)
    .await
    .map_err(|e| {
        tracing::error!("authz: failed to check organisation visibility: {e}");
        tonic::Status::internal("failed to check organisation visibility")
    })
}

async fn check_org_access(
    db: &PgPool,
    actor: &Actor,
    organisation_id: Uuid,
    required_role: OrgRole,
) -> Result<AuthzContext, tonic::Status> {
    match actor {
        Actor::ServiceAccount { .. } => Ok(AuthzContext {
            actor: actor.clone(),
            organisation_id,
        }),
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
            .ok_or_else(|| tonic::Status::permission_denied("not a member of this organisation"))?;

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

pub async fn require_intent_access(
    db: &sqlx::PgPool,
    actor: &crate::actor::Actor,
    release_intent_id: Uuid,
) -> Result<(), tonic::Status> {
    let org = sqlx::query_scalar!(
        "SELECT p.organisation FROM release_intents ri
         JOIN projects p ON p.id = ri.project_id
         WHERE ri.id = $1",
        release_intent_id,
    )
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!("authz: resolve intent org failed: {e}");
        tonic::Status::internal("authorization lookup failed")
    })?
    .ok_or_else(|| tonic::Status::not_found("release intent not found"))?;

    require_org_access(db, actor, &org, OrgRole::Member).await?;
    Ok(())
}

#[cfg(test)]
mod typed_gate_tests {
    use super::{AuthenticatedActor, UnauthenticatedActor};
    use crate::actor::Actor;
    use uuid::Uuid;

    fn user(id: Uuid) -> AuthenticatedActor {
        UnauthenticatedActor(Some(Actor::User { user_id: id }))
            .require_authenticated()
            .expect("user actor is authenticated")
    }
    fn service_account() -> AuthenticatedActor {
        UnauthenticatedActor(Some(Actor::ServiceAccount {
            service_account_id: Uuid::now_v7(),
        }))
        .require_authenticated()
        .expect("service account is authenticated")
    }
    fn app() -> AuthenticatedActor {
        UnauthenticatedActor(Some(Actor::App {
            app_id: Uuid::now_v7(),
            organisation_id: Uuid::now_v7(),
            scopes: None,
        }))
        .require_authenticated()
        .expect("app is authenticated")
    }

    #[test]
    fn require_authenticated_passes_when_actor_present() {
        let id = Uuid::now_v7();
        let unauth = UnauthenticatedActor(Some(Actor::User { user_id: id }));
        assert!(unauth.require_authenticated().is_ok());
    }

    #[test]
    fn require_authenticated_returns_401_when_missing() {
        let unauth = UnauthenticatedActor(None);
        let err = unauth.require_authenticated().unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn user_self_or_sa_allows_user_matching_target() {
        let id = Uuid::now_v7();
        assert!(user(id).require_user_self_or_service_account(id).is_ok());
    }

    #[test]
    fn user_self_or_sa_denies_user_with_other_target() {
        let me = Uuid::now_v7();
        let target = Uuid::now_v7();
        let err = user(me)
            .require_user_self_or_service_account(target)
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn user_self_or_sa_allows_service_account_for_any_target() {
        let target = Uuid::now_v7();
        assert!(
            service_account()
                .require_user_self_or_service_account(target)
                .is_ok()
        );
    }

    #[test]
    fn user_self_or_sa_denies_app_actor() {
        let target = Uuid::now_v7();
        assert!(app().require_user_self_or_service_account(target).is_err());
    }

    #[test]
    fn require_user_self_denies_service_account() {
        let target = Uuid::now_v7();
        let err = service_account().require_user_self(target).unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn require_service_account_denies_user() {
        let id = Uuid::now_v7();
        let err = user(id).require_service_account().unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[test]
    fn into_actor_escape_hatch_returns_inner() {
        let id = Uuid::now_v7();
        match user(id).into_actor() {
            Actor::User { user_id } => assert_eq!(user_id, id),
            _ => panic!("expected User variant"),
        }
    }
    #[tokio::test]
    async fn app_token_cannot_inherit_operator_privileges() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/unused")
            .unwrap();
        let actor = Actor::App {
            app_id: Uuid::now_v7(),
            organisation_id: Uuid::now_v7(),
            scopes: None,
        };
        let error = super::require_operator_access(&pool, &actor, "org")
            .await
            .err()
            .expect("app token must not get operator access");
        assert_eq!(error.code(), tonic::Code::PermissionDenied);
    }
}
