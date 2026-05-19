use std::net::SocketAddr;

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

pub struct ServeHttp {
    pub addr: SocketAddr,
    pub state: AppState,
}

impl Component for ServeHttp {
    fn info(&self) -> ComponentInfo {
        "forage/http".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let app = crate::build_router(self.state.clone());

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
