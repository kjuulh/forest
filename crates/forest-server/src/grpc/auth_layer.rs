use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tower::{Layer, Service};

use crate::{
    state::State,
    tokens::{AccessToken, TokenServiceState},
};

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

            let access_token = match AccessToken::new_from(&token) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "invalid access token");
                    return Ok(grpc_unauthenticated(&format!("invalid token: {e}")));
                }
            };

            let claims = match state.tokens().verify_access_token(&access_token) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "token verification failed");
                    return Ok(grpc_unauthenticated(&format!(
                        "token verification failed: {e}"
                    )));
                }
            };

            req.extensions_mut().insert(claims);
            inner.call(req).await
        })
    }
}
