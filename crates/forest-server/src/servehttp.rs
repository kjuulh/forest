use std::net::SocketAddr;

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub struct ServeHttp {
    pub host: SocketAddr,
}

impl Component for ServeHttp {
    fn info(&self) -> ComponentInfo {
        "forest-server/http".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let router = axum::Router::new().merge(nostatus::axum_routes(nostatus::global()));

        let listener = TcpListener::bind(&self.host)
            .await
            .context("failed to bind to port")?;

        axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled();
            })
            .await
            .context("http server failed")?;

        Ok(())
    }
}
