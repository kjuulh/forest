use std::{
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    task::{Context, Poll},
};

use anyhow::Context as AnyhowContext;
use forest_grpc_interface::{
    TokenInfoRequest, users_service_client::UsersServiceClient, RefreshTokenRequest,
};
use tokio::sync::{Mutex, OnceCell};
use tonic::transport::{Channel, ClientTlsConfig};
use tower::{Layer, Service};

use crate::{
    state::State,
    user_state::{UserState, UserStateLoader, UserStateLoaderState, compute_refresh_after},
};

/// How often (in seconds) the interceptor validates the token against the server.
const VALIDATION_INTERVAL_SECS: i64 = 60;

fn validated_stamp_path() -> PathBuf {
    std::env::temp_dir().join("forest-token-validated")
}

fn read_last_validated() -> i64 {
    std::fs::read_to_string(validated_stamp_path())
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

fn write_last_validated(ts: i64) {
    let _ = std::fs::write(validated_stamp_path(), ts.to_string());
}

#[derive(Clone)]
pub struct AuthMiddlewareLayer {
    state: UserStateLoader,
    host: String,
    refresh_channel: Arc<OnceCell<Channel>>,
    refresh_lock: Arc<Mutex<()>>,
    /// In-process cache so we only hit the temp file once per process lifetime.
    last_validated: Arc<AtomicI64>,
}

impl<S> Layer<S> for AuthMiddlewareLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        AuthMiddleware {
            inner: service,
            state: self.state.clone(),
            host: self.host.clone(),
            refresh_channel: self.refresh_channel.clone(),
            refresh_lock: self.refresh_lock.clone(),
            last_validated: self.last_validated.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    state: UserStateLoader,
    host: String,
    refresh_channel: Arc<OnceCell<Channel>>,
    refresh_lock: Arc<Mutex<()>>,
    last_validated: Arc<AtomicI64>,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for AuthMiddleware<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + std::error::Error + Send + Sync + 'static,
    ReqBody: Send + 'static,
    ResBody: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map(|r| r.map_err(Into::into))
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        // See: https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let loader = self.state.clone();
        let host = self.host.clone();
        let refresh_channel = self.refresh_channel.clone();
        let refresh_lock = self.refresh_lock.clone();
        let last_validated = self.last_validated.clone();

        Box::pin(async move {
            let mut user_state = loader
                .get_state()
                .await
                .map_err(|e| -> BoxError { format!("user is not logged in: {e}").into() })?
                .ok_or_else(|| -> BoxError { "user is not logged in".into() })?;

            // Check if token needs refreshing (at the midpoint of token lifetime)
            let needs_refresh = match user_state.refresh_after {
                Some(refresh_after) => chrono::Utc::now().timestamp() >= refresh_after,
                None => true, // No expiry stored (legacy state) — refresh to be safe
            };

            if needs_refresh {
                // Acquire in-process lock to prevent concurrent refresh attempts.
                let _guard = refresh_lock.lock().await;

                // Re-read state: another request may have already refreshed while we waited.
                let (locked_state, file_lock) = loader
                    .read_locked()
                    .await
                    .map_err(|e| -> BoxError { format!("failed to read state: {e}").into() })?;

                let current = locked_state
                    .ok_or_else(|| -> BoxError { "user is not logged in".into() })?;

                let still_needs_refresh = match current.refresh_after {
                    Some(refresh_after) => chrono::Utc::now().timestamp() >= refresh_after,
                    None => true,
                };

                if still_needs_refresh {
                    tracing::debug!("access token needs refresh, refreshing");

                    match do_refresh(
                        &host,
                        &refresh_channel,
                        &loader,
                        &current,
                        &file_lock,
                    )
                    .await
                    {
                        Ok(new_state) => {
                            user_state = new_state;
                            let ts = chrono::Utc::now().timestamp();
                            last_validated.store(ts, Ordering::Relaxed);
                            write_last_validated(ts);
                        }
                        Err(e) => {
                            tracing::warn!("token refresh failed: {e:#}");
                            // Continue with the existing token — it may still be valid
                            // even though the refresh window has passed.
                            user_state = current;
                        }
                    }
                } else {
                    // Another request already refreshed.
                    user_state = current;
                }
            } else {
                // Token is within its expected lifetime — periodically validate it
                // against the server to catch revocations or key rotations.
                let now = chrono::Utc::now().timestamp();
                let mut last = last_validated.load(Ordering::Relaxed);
                if last == 0 {
                    // First call in this process — seed from the temp file.
                    last = read_last_validated();
                    last_validated.store(last, Ordering::Relaxed);
                }

                if now - last >= VALIDATION_INTERVAL_SECS {
                    tracing::debug!("periodic token validation check");

                    match validate_token(
                        &host,
                        &refresh_channel,
                        &user_state.access_token,
                    )
                    .await
                    {
                        Ok(()) => {
                            last_validated.store(now, Ordering::Relaxed);
                            write_last_validated(now);
                        }
                        Err(e) => {
                            tracing::debug!("token validation failed ({e:#}), attempting refresh");

                            let _guard = refresh_lock.lock().await;
                            let (locked_state, file_lock) = loader
                                .read_locked()
                                .await
                                .map_err(|e| -> BoxError {
                                    format!("failed to read state: {e}").into()
                                })?;

                            if let Some(current) = locked_state {
                                match do_refresh(
                                    &host,
                                    &refresh_channel,
                                    &loader,
                                    &current,
                                    &file_lock,
                                )
                                .await
                                {
                                    Ok(new_state) => {
                                        user_state = new_state;
                                        let ts = chrono::Utc::now().timestamp();
                                        last_validated.store(ts, Ordering::Relaxed);
                                        write_last_validated(ts);
                                    }
                                    Err(e) => {
                                        tracing::warn!("token refresh after failed validation: {e:#}");
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let mut req = req;
            req.headers_mut().insert(
                http::header::AUTHORIZATION,
                http::HeaderValue::from_str(&format!("Bearer {}", user_state.access_token))
                    .map_err(|e| -> BoxError { e.into() })?,
            );

            let response = inner.call(req).await.map_err(Into::into)?;
            Ok(response)
        })
    }
}

/// Validate the current access token by calling the server's TokenInfo RPC.
/// Returns Ok(()) if the token is valid, Err if it's rejected.
async fn validate_token(
    host: &str,
    channel: &Arc<OnceCell<Channel>>,
    access_token: &str,
) -> anyhow::Result<()> {
    let ch = get_or_connect(host, channel).await?;
    let mut client = UsersServiceClient::new(ch);

    let mut request = tonic::Request::new(TokenInfoRequest {});
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", access_token)
            .parse()
            .context("invalid token header value")?,
    );

    client
        .token_info(request)
        .await
        .context("token validation")?;

    Ok(())
}

async fn get_or_connect(
    host: &str,
    channel: &Arc<OnceCell<Channel>>,
) -> anyhow::Result<Channel> {
    let ch = channel
        .get_or_try_init(|| async {
            Channel::from_shared(host.to_owned())
                .context("invalid host")?
                .tls_config(ClientTlsConfig::new().with_enabled_roots())
                .context("tls config")?
                .connect()
                .await
                .context("connect for refresh")
        })
        .await?
        .clone();
    Ok(ch)
}

async fn do_refresh(
    host: &str,
    refresh_channel: &Arc<OnceCell<Channel>>,
    loader: &UserStateLoader,
    current: &UserState,
    file_lock: &std::fs::File,
) -> anyhow::Result<UserState> {
    let channel = get_or_connect(host, refresh_channel).await?;

    let mut client = UsersServiceClient::new(channel);

    let resp = client
        .refresh_token(RefreshTokenRequest {
            refresh_token: current.refresh_access.clone(),
        })
        .await
        .context("refresh token RPC")?;

    let tokens = resp
        .into_inner()
        .tokens
        .context("no tokens in refresh response")?;

    let now = chrono::Utc::now().timestamp();
    let refresh_after = compute_refresh_after(now, tokens.expires_in_seconds);

    let new_state = UserState {
        user_id: current.user_id.clone(),
        username: current.username.clone(),
        emails: current.emails.clone(),
        access_token: tokens.access_token,
        refresh_access: tokens.refresh_token,
        refresh_after: Some(refresh_after),
    };

    loader.write_locked(&new_state, file_lock).await?;

    tracing::debug!("tokens refreshed successfully");

    Ok(new_state)
}

pub trait AuthMiddlewareLayerState {
    fn auth_interceptor(&self) -> AuthMiddlewareLayer;
}

impl AuthMiddlewareLayerState for State {
    fn auth_interceptor(&self) -> AuthMiddlewareLayer {
        AuthMiddlewareLayer {
            state: self.user_state(),
            host: self.config.forest_server.clone(),
            refresh_channel: Arc::new(OnceCell::const_new()),
            refresh_lock: Arc::new(Mutex::new(())),
            last_validated: Arc::new(AtomicI64::new(0)),
        }
    }
}
