use std::{
    pin::Pin,
    task::{Context, Poll},
};

use sha2::Digest;
use tower::{Layer, Service};

use crate::{
    actor::Actor,
    services::oauth_apps::{OAuthAppServiceState, SCOPE_COMPONENTS_PUBLISH},
    state::State,
    tokens::{AccessToken, TokenServiceState},
};

async fn resolve_personal_access_token(
    db: &sqlx::PgPool,
    raw_token: &str,
) -> anyhow::Result<Option<Actor>> {
    let token_hash = sha2::Sha256::digest(raw_token.as_bytes()).to_vec();

    let row = sqlx::query!(
        r#"
        SELECT id, user_id
        FROM personal_access_tokens
        WHERE token_hash = $1
          AND (expires_at IS NULL OR expires_at > now())
        "#,
        &token_hash
    )
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Touch last_used
    sqlx::query!(
        "UPDATE personal_access_tokens SET last_used = now() WHERE id = $1",
        row.id
    )
    .execute(db)
    .await
    .ok();

    Ok(Some(Actor::User {
        user_id: row.user_id,
    }))
}

async fn resolve_app_token(db: &sqlx::PgPool, raw_token: &str) -> anyhow::Result<Option<Actor>> {
    let token_hash = sha2::Sha256::digest(raw_token.as_bytes()).to_vec();

    let row = sqlx::query!(
        r#"
        SELECT at.app_id, a.organisation_id
        FROM app_tokens at
        JOIN apps a ON at.app_id = a.id
        WHERE at.token_hash = $1
          AND at.revoked = false
          AND a.suspended = false
          AND (at.expires_at IS NULL OR at.expires_at > now())
        "#,
        &token_hash
    )
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Touch last_used
    sqlx::query!(
        "UPDATE app_tokens SET last_used = now() WHERE token_hash = $1",
        &token_hash
    )
    .execute(db)
    .await
    .ok();

    Ok(Some(Actor::App {
        app_id: row.app_id,
        organisation_id: row.organisation_id,
        scopes: None,
    }))
}

/// The gRPC methods an OAuth client-credentials token may call, by scope.
///
/// Deny by default. Such a token authenticates as its app, and an app passes
/// every org-member check in its own organisation, so without this list a
/// token granted only `directory:read` could create releases or edit
/// destinations. A scope not listed here grants no gRPC method at all.
///
/// `components:publish` covers what `forest publish` calls: the registry
/// (reading the org's components, and the upload, manifest, commit, abort
/// and unpublish writes, which also check the scope themselves), plus
/// `CreateProject`/`UpdateProject`, because publishing ensures the
/// component's project exists and records its metadata.
const SCOPED_METHODS: &[(&str, &[&str])] = &[(
    SCOPE_COMPONENTS_PUBLISH,
    &[
        "/forest.v1.RegistryService/",
        "/forest.v1.ReleaseService/CreateProject",
        "/forest.v1.ReleaseService/UpdateProject",
    ],
)];

/// Whether a token holding `scopes` may call the gRPC method at `path`.
pub(crate) fn scoped_token_permits(path: &str, scopes: &[String]) -> bool {
    SCOPED_METHODS.iter().any(|(scope, methods)| {
        scopes.iter().any(|s| s == scope)
            && methods.iter().any(|m| {
                if m.ends_with('/') {
                    path.starts_with(m)
                } else {
                    path == *m
                }
            })
    })
}

/// Compare the incoming token against the pre-hashed service account key
/// held in memory (loaded from `FOREST_SERVICE_ACCOUNT_API_KEY` at startup).
fn resolve_service_account_token(expected_hash: Option<&[u8]>, raw_token: &str) -> Option<Actor> {
    let expected = expected_hash?;
    let incoming = sha2::Sha256::digest(raw_token.as_bytes());
    if constant_time_eq(incoming.as_slice(), expected) {
        Some(Actor::ServiceAccount {
            service_account_id: uuid::Uuid::nil(),
        })
    } else {
        None
    }
}

/// Constant-time comparison to avoid timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[derive(Clone)]
pub struct AuthMiddlewareLayer {
    state: State,
}

impl AuthMiddlewareLayer {
    pub fn new(state: State) -> Self {
        Self { state }
    }
}

impl<S> Layer<S> for AuthMiddlewareLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        AuthMiddleware {
            inner: service,
            state: self.state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    state: State,
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    /// Token required; reject if missing or invalid.
    Required,
    /// Token optional; if present and valid, extract Actor; otherwise pass through anonymously.
    /// Used for registry browsing where private components must be visible to their org members
    /// while anonymous users can still see public components.
    Optional,
    /// Skip token handling entirely.
    None,
}

fn auth_mode(path: &str) -> AuthMode {
    let none = [
        "/forest.v1.UsersService/Register",
        "/forest.v1.UsersService/Login",
        "/forest.v1.UsersService/RefreshToken",
        "/forest.v1.UsersService/VerifyLoginMfa",
        // Device authorization grant: the CLI has no token yet, that's
        // the whole point. Approve/Deny are NOT on this list — they
        // require service-account auth and are checked at the handler
        // level via `require_service_account()`.
        "/forest.v1.UsersService/InitiateDeviceLogin",
        "/forest.v1.UsersService/PollDeviceLogin",
        "/forest.v1.StatusService/",
        // Runner service uses release-scoped tokens, not JWT
        "/forest.v1.RunnerService/",
        // Standard gRPC health protocol — used by the ALB / k8s probes.
        // Probers can't carry user credentials; refusing them makes the
        // server look dead. The health namespace exposes only liveness, so
        // it's safe to leave open.
        "/grpc.health.v1.Health/",
        // Public registry endpoints — by design these never read the
        // caller's identity and always filter to public components, so
        // skipping token handling closes off any chance of escalating a
        // misconfigured caller token into private reads.
        "/forest.v1.RegistryService/SearchPublicComponents",
        "/forest.v1.RegistryService/GetPublicComponentDetail",
        "/forest.v1.RegistryService/GetPublicComponentManifest",
    ];
    let optional = [
        // Registry discovery endpoints allow anonymous browsing of public components,
        // but must still recognise authenticated callers so they see private components
        // from their own orgs.
        "/forest.v1.RegistryService/SearchComponents",
        "/forest.v1.RegistryService/GetComponentDetail",
    ];
    if none.iter().any(|p| path.starts_with(p)) {
        AuthMode::None
    } else if optional.iter().any(|p| path.starts_with(p)) {
        AuthMode::Optional
    } else {
        AuthMode::Required
    }
}

fn grpc_unauthenticated<B: Default>(message: &str) -> http::Response<B> {
    grpc_status_response(16, message)
}

fn grpc_permission_denied<B: Default>(message: &str) -> http::Response<B> {
    grpc_status_response(7, message)
}

fn grpc_status_response<B: Default>(code: u16, message: &str) -> http::Response<B> {
    let mut response = http::Response::new(B::default());
    response
        .headers_mut()
        .insert("grpc-status", http::HeaderValue::from(code));
    response.headers_mut().insert(
        "content-type",
        http::HeaderValue::from_static("application/grpc"),
    );
    if let Ok(msg) = http::HeaderValue::from_str(message) {
        response.headers_mut().insert("grpc-message", msg);
    }
    response
}

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for AuthMiddleware<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<ReqBody>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let state = self.state.clone();

        Box::pin(async move {
            let path = req.uri().path().to_owned();
            let mode = auth_mode(&path);

            if mode == AuthMode::None {
                return inner.call(req).await;
            }

            let token = match req
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
            {
                Some(t) => t.to_owned(),
                None => {
                    if mode == AuthMode::Optional {
                        return inner.call(req).await;
                    }
                    tracing::warn!(path = %path, "missing authorization token");
                    return Ok(grpc_unauthenticated("missing authorization token"));
                }
            };

            // 1. Try JWT (user tokens)
            let access_token = AccessToken::new_from(&token);
            if let Ok(access_token) = access_token
                && let Ok(claims) = state.tokens().verify_access_token(&access_token)
            {
                let user_id: uuid::Uuid = match claims.user_id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        return Ok(grpc_unauthenticated("invalid user_id in token"));
                    }
                };
                let actor = Actor::User { user_id };
                req.extensions_mut().insert(actor);
                req.extensions_mut().insert(claims);
                return inner.call(req).await;
            }

            // 2. Try service account API key (in-memory, no DB hit)
            if let Some(actor) = resolve_service_account_token(
                state.config.service_account_token_hash.as_deref(),
                &token,
            ) {
                req.extensions_mut().insert(actor);
                return inner.call(req).await;
            }

            // 3. Try personal access token (DB — resolves to Actor::User)
            match resolve_personal_access_token(&state.db, &token).await {
                Ok(Some(actor)) => {
                    req.extensions_mut().insert(actor);
                    return inner.call(req).await;
                }
                Ok(None) => {} // not a PAT, try next
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "PAT lookup failed");
                }
            }

            // 4. Try a legacy app token (DB)
            match resolve_app_token(&state.db, &token).await {
                Ok(Some(actor)) => {
                    req.extensions_mut().insert(actor);
                    return inner.call(req).await;
                }
                Ok(None) => {} // not an app token, try next
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "app token lookup failed");
                }
            }

            // 5. Fall back to an OAuth client-credentials token (DB), limited
            //    to the methods its scopes cover.
            match state
                .oauth_app_service()
                .introspect_client_token(&token)
                .await
            {
                Ok(Some(principal)) => {
                    if !scoped_token_permits(&path, &principal.scopes) {
                        tracing::warn!(path = %path, app_id = %principal.app_id, "client token's scopes do not cover this method");
                        return Ok(grpc_permission_denied(
                            "token's scopes do not cover this method",
                        ));
                    }
                    req.extensions_mut().insert(Actor::App {
                        app_id: principal.app_id,
                        organisation_id: principal.organisation_id,
                        scopes: Some(principal.scopes),
                    });
                    inner.call(req).await
                }
                Ok(None) => {
                    if mode == AuthMode::Optional {
                        return inner.call(req).await;
                    }
                    tracing::warn!(path = %path, "token verification failed: no matching user, app, client, or service account token");
                    Ok(grpc_unauthenticated("token verification failed"))
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "client token lookup failed");
                    Ok(grpc_unauthenticated("token verification failed"))
                }
            }
        })
    }
}

#[cfg(test)]
mod scoped_token_tests {
    use super::scoped_token_permits;

    fn scopes(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_publish_grant_reaches_the_registry_and_project_upsert() {
        let s = scopes(&["components:publish"]);
        for path in [
            "/forest.v1.RegistryService/BeginUpload",
            "/forest.v1.RegistryService/UploadBinary",
            "/forest.v1.RegistryService/PublishManifest",
            "/forest.v1.RegistryService/CommitUpload",
            "/forest.v1.RegistryService/AbortUpload",
            "/forest.v1.ReleaseService/CreateProject",
            "/forest.v1.ReleaseService/UpdateProject",
        ] {
            assert!(scoped_token_permits(path, &s), "{path} should be allowed");
        }
    }

    /// The point of the list: an app passes org-member checks, so anything
    /// not listed must be refused here.
    #[test]
    fn the_publish_grant_reaches_nothing_else() {
        let s = scopes(&["components:publish"]);
        for path in [
            "/forest.v1.ReleaseService/CreateReleaseIntent",
            "/forest.v1.ReleaseService/CreateProjectTrigger",
            "/forest.v1.DestinationService/CreateDestination",
            "/forest.v1.OrganisationService/AddMember",
            "/forest.v1.OAuthAppsService/CreateOAuthApp",
            "/forest.v1.RegistryServiceX/BeginUpload",
        ] {
            assert!(!scoped_token_permits(path, &s), "{path} should be refused");
        }
    }

    #[test]
    fn a_scope_without_methods_grants_no_method() {
        let s = scopes(&["directory:read", "openid", "profile", "email"]);
        assert!(!scoped_token_permits(
            "/forest.v1.RegistryService/BeginUpload",
            &s
        ));
        assert!(!scoped_token_permits(
            "/forest.v1.ReleaseService/CreateProject",
            &s
        ));
    }

    #[test]
    fn no_scopes_grant_no_method() {
        assert!(!scoped_token_permits(
            "/forest.v1.RegistryService/BeginUpload",
            &[]
        ));
    }
}
