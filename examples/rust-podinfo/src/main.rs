use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use notmad::{Component, ComponentInfo, MadError};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Clone)]
struct AppState {
    started_at: Instant,
}

#[derive(Serialize)]
struct InfoResponse {
    name: &'static str,
    version: &'static str,
    hostname: String,
    uptime_seconds: u64,
}

#[derive(Serialize)]
struct VersionResponse {
    version: &'static str,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct EnvResponse {
    env: Vec<EnvEntry>,
}

#[derive(Serialize)]
struct EnvEntry {
    key: String,
    value: String,
}

async fn info_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let hostname = hostname();
    Json(InfoResponse {
        name: NAME,
        version: VERSION,
        hostname,
        uptime_seconds: state.started_at.elapsed().as_secs(),
    })
}

async fn healthz_handler() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

async fn readyz_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(HealthResponse { status: "ready" }))
}

async fn version_handler() -> impl IntoResponse {
    Json(VersionResponse { version: VERSION })
}

async fn env_handler() -> impl IntoResponse {
    let env: Vec<EnvEntry> = std::env::vars()
        .filter(|(key, _)| {
            key.starts_with("RUST_LOG") || key.starts_with("PODINFO_") || key.starts_with("FOREST_")
        })
        .map(|(key, value)| EnvEntry { key, value })
        .collect();
    Json(EnvResponse { env })
}

fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    for arg in std::env::args() {
        if arg == "--help" {
            println!("{NAME} v{VERSION}");
            println!("  A podinfo-like service managed by forest");
            println!();
            println!("Ports:");
            println!("  :8080  external (info, version, env)");
            println!("  :8081  internal (healthz, readyz)");
            return Ok(());
        }
    }

    notmad::Mad::builder()
        .add(HttpServer {
            name: "external".into(),
            addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 8080),
            kind: ServerKind::External,
        })
        .add(HttpServer {
            name: "internal".into(),
            addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 8081),
            kind: ServerKind::Internal,
        })
        .run()
        .await?;

    Ok(())
}

#[derive(Clone)]
enum ServerKind {
    External,
    Internal,
}

struct HttpServer {
    name: String,
    addr: SocketAddr,
    kind: ServerKind,
}

impl Component for HttpServer {
    fn info(&self) -> ComponentInfo {
        format!("{}:{}", self.name, self.addr).into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let state = Arc::new(AppState {
            started_at: Instant::now(),
        });

        let router = match self.kind {
            ServerKind::External => axum::Router::new()
                .route("/", get(info_handler))
                .route("/version", get(version_handler))
                .route("/env", get(env_handler))
                .with_state(state),
            ServerKind::Internal => axum::Router::new()
                .route("/healthz", get(healthz_handler))
                .route("/readyz", get(readyz_handler))
                .with_state(state),
        };

        let listener = TcpListener::bind(self.addr).await.context("bind to port")?;

        tracing::info!(name = self.name, addr = %self.addr, "starting server");

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled().await;
            })
            .await
            .context("http server failed")?;

        Ok(())
    }
}
