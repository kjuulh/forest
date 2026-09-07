use std::net::SocketAddr;

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use crate::templates::TemplateEngine;

pub enum ServeMode {
    Application(AppState),
    Maintenance(TemplateEngine),
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
        let app = match &self.mode {
            ServeMode::Application(state) => crate::build_router(state.clone()),
            ServeMode::Maintenance(templates) => crate::build_maintenance_router(templates.clone()),
        };
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
