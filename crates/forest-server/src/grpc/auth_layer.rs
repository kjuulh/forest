use std::{
    pin::Pin,
    task::{Context, Poll},
};

use sha2::Digest;
use tower::{Layer, Service};

use crate::{
    actor::Actor,
    state::State,
    tokens::{AccessToken, TokenServiceState},
};

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
    }))
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

/// Paths that do not require authentication.
fn requires_auth(path: &str) -> bool {
    let unauthenticated = [
        "/forest.v1.UsersService/Register",
        "/forest.v1.UsersService/Login",
        "/forest.v1.UsersService/RefreshToken",
        "/forest.v1.StatusService/",
        // Runner service uses release-scoped tokens, not JWT
        "/forest.v1.RunnerService/",
    ];
    !unauthenticated.iter().any(|p| path.starts_with(p))
}

fn grpc_unauthenticated<B: Default>(message: &str) -> http::Response<B> {
    let mut response = http::Response::new(B::default());
    response
        .headers_mut()
        .insert("grpc-status", http::HeaderValue::from(16));
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

            if !requires_auth(&path) {
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
                    tracing::warn!(path = %path, "missing authorization token");
                    return Ok(grpc_unauthenticated("missing authorization token"));
                }
            };

            // Try JWT first (user tokens)
            let access_token = AccessToken::new_from(&token);
            if let Ok(access_token) = access_token {
                if let Ok(claims) = state.tokens().verify_access_token(&access_token) {
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
            }

            // Fall back to app token lookup
            match resolve_app_token(&state.db, &token).await {
                Ok(Some(actor)) => {
                    req.extensions_mut().insert(actor);
                    inner.call(req).await
                }
                Ok(None) => {
                    tracing::warn!(path = %path, "token verification failed: no matching user or app token");
                    Ok(grpc_unauthenticated("token verification failed"))
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "app token lookup failed");
                    Ok(grpc_unauthenticated("token verification failed"))
                }
            }
        })
    }
}
