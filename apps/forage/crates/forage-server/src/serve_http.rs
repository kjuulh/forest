use std::net::SocketAddr;

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::templates::TemplateEngine;

/// What the HTTP listener serves. Chosen once at startup from
/// `FORAGE_MAINTENANCE_MODE`.
pub enum ServeMode {
    /// The normal application router.
    Application(Box<AppState>),
    /// Every route answers 503 with the maintenance page; health probes stay
    /// up so the orchestrator does not restart a replica that is healthy.
    Maintenance(Box<TemplateEngine>),
}

impl ServeMode {
    pub fn router(&self) -> axum::Router {
        match self {
            ServeMode::Application(state) => crate::build_router((**state).clone()),
            ServeMode::Maintenance(templates) => {
                crate::build_maintenance_router((**templates).clone())
            }
        }
    }
}

pub struct ServeHttp {
    pub addr: SocketAddr,
    pub mode: ServeMode,
}

impl Component for ServeHttp {
    fn info(&self) -> ComponentInfo {
        "forage/http".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let app = self.mode.router();
        let listener = tokio::net::TcpListener::bind(self.addr)
            .await
            .context(anyhow::anyhow!("failed to listen on port: {}", self.addr))?;

        tracing::info!("listening on {}", self.addr);

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled().await;
            })
            .await
            .context("failed to run axum server")?;

        Ok(())
    }
}
