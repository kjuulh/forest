use std::net::SocketAddr;

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::object_store::ObjectStore;

pub struct ServeHttp {
    pub host: SocketAddr,
    pub object_store: ObjectStore,
}

impl Component for ServeHttp {
    fn info(&self) -> ComponentInfo {
        "forest-server/http".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let router = axum::Router::new()
            .merge(nostatus::axum_routes(nostatus::global()))
            .merge(crate::oci_registry::oci_routes(self.object_store.clone()));

        let listener = TcpListener::bind(&self.host)
            .await
            .context("failed to bind to port")?;

        tracing::info!("OCI registry available at http://{}/v2/", self.host);

        axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled().await;
            })
            .await
            .context("http server failed")?;

        Ok(())
    }
}
