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

// ─── Typed authz gate (preferred over the free functions above) ──────
//
// Use this for any new handler. Three stages, each consuming the
// previous, so the type system forces the handler down the gate:
//
//     UnauthenticatedActor   (what middleware injected)
//          │
//          │   .require_authenticated()?     // 401 if missing
//          ▼
//     AuthenticatedActor      (someone is logged in)
//          │
//          │   .require_user_self_or_service_account(target)?   // or
//          │   .require_service_account()?                       // or
//          │   .require_user_self(target)?                       // or
//          │   .into_actor()       // escape hatch for read-any-auth
//          ▼
//        Actor                (use it)
//
// You cannot read the underlying Actor without going through the gate,
// which makes "I forgot the authz check" a compile-time-detectable
// mistake (#[must_use] catches the warning) rather than a runtime CVE.
//
// **Known limitation** (caught by the second adversarial review): the
// `#[must_use]` annotations on `UnauthenticatedActor` and
// `AuthenticatedActor` warn only for bare-expression drops like
// `unauthenticated_actor(&request);`. They do **not** catch
// `let _ = unauthenticated_actor(&request);` or
// `let _auth = unauth.require_authenticated()?;` (where the
// `AuthenticatedActor` is dropped midway). New handlers should chain
// the gate through to its terminal stage in a single expression:
//
//   let _actor = crate::grpc::authorize::unauthenticated_actor(&request)
//       .require_authenticated()?
//       .require_user_self_or_service_account(user_id)?;
//
// Reviewing new handlers via this file's PR diff is the recommended
// defense against the bypass — there is no compiler-enforced way to
// prevent a future author from writing `let _ = ...` and dropping the
// gate entirely.

/// An actor extracted from a tonic request that has NOT yet been
/// verified as authenticated. The only way to read the inner [`Actor`]
/// is via [`UnauthenticatedActor::require_authenticated`].
///
/// The middleware in `auth_layer.rs` rejects unauthenticated requests
/// for all RPCs not on the whitelist (Register / Login / RefreshToken /
/// VerifyLoginMfa / Status / Runner / Health). The check here is
/// defense-in-depth: it shields handlers from a future middleware
/// misconfiguration where a new RPC accidentally falls through the
/// `AuthMode::Required` net.
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

/// An actor known to be authenticated but not yet authorised for the
/// specific resource the handler operates on. Call one of the
/// `require_*` methods to convert into the underlying [`Actor`], or
/// `into_actor` for read endpoints where any authenticated caller is OK.
#[derive(Debug)]
#[must_use = "extract an AuthenticatedActor only to use it — call a require_* method or into_actor"]
pub struct AuthenticatedActor(Actor);

impl AuthenticatedActor {
    /// User-self path: the caller is permitted iff they ARE the target
    /// user or a service account acting on their behalf. The standard
    /// gate for user-scoped write paths (update_user, change_password,
    /// add_email, etc.).
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

    /// Service-account-only path (OAuthLogin, ConfirmEmailVerification).
    pub fn require_service_account(self) -> Result<Actor, tonic::Status> {
        match self.0 {
            Actor::ServiceAccount { .. } => Ok(self.0),
            _ => Err(tonic::Status::permission_denied(
                "this operation requires service account authentication",
            )),
        }
    }

    /// User-self only, no service-account bypass. Use for endpoints
    /// where the service account has no legitimate need to act on
    /// behalf of a specific user (e.g. UnlinkOAuthProvider — Forage's
    /// signup-time service account has no business unlinking).
    pub fn require_user_self(self, target_user_id: Uuid) -> Result<Actor, tonic::Status> {
        match self.0 {
            Actor::User { user_id } if user_id == target_user_id => Ok(self.0),
            _ => Err(tonic::Status::permission_denied(
                "operation restricted to the target user",
            )),
        }
    }

    /// Escape hatch for read endpoints where any authenticated caller
    /// is acceptable (e.g. ListUsers, GetUserStats — gate behind any
    /// session but don't restrict by user_id).
    pub fn into_actor(self) -> Actor {
        self.0
    }
}

/// Extract the unauthenticated actor wrapper from a tonic request.
/// This is the **only** entry point for the typed gate — new handlers
/// should call this and then walk the gate stages above. Old call
/// sites can continue to use [`extract_actor`] / [`try_extract_actor`]
/// during the migration.
pub fn unauthenticated_actor<T>(request: &tonic::Request<T>) -> UnauthenticatedActor {
    UnauthenticatedActor(request.extensions().get::<Actor>().cloned())
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
        // Apps are org-scoped; users.rs operations are not their concern.
        let target = Uuid::now_v7();
        assert!(
            app()
                .require_user_self_or_service_account(target)
                .is_err()
        );
    }

    #[test]
    fn require_user_self_denies_service_account() {
        // The user-self-only path closes the SA bypass; used by
        // UnlinkOAuthProvider where SA has no legitimate need.
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
        // For read-any-auth endpoints. Confirms the variant survives.
        let id = Uuid::now_v7();
        match user(id).into_actor() {
            Actor::User { user_id } => assert_eq!(user_id, id),
            _ => panic!("expected User variant"),
        }
    }
}
