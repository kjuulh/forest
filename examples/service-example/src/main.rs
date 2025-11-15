use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context;
use async_trait::async_trait;
use axum::{Router, routing::get};
use notmad::{Component, MadError};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt
        // .with_env_filter(
        //     EnvFilter::from_default_env().add_directive("notmad=debug".parse().context("notmad")?),
        // )
        ::init();

    for arg in std::env::args() {
        if arg == "--help" {
            println!("run without anything to serve");
            return Ok(());
        }
    }

    notmad::Mad::builder()
        .add(create_router(
            "external",
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3000),
        ))
        .add(create_router(
            "internal",
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3001),
        ))
        .add(create_router(
            "grpc_external",
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 4000),
        ))
        .add(create_router(
            "grpc_internal",
            SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 4001),
        ))
        .run()
        .await?;

    Ok(())
}

fn create_router(name: &str, port: SocketAddr) -> Http {
    Http {
        name: name.into(),
        port,
    }
}

struct Http {
    name: String,
    port: SocketAddr,
}

#[async_trait]
impl Component for Http {
    fn name(&self) -> Option<String> {
        Some(format!("{}:{}", self.name, self.port))
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let router = axum::Router::new().route(
            "/",
            get({
                let name = self.name.clone();
                async move || -> String { name.clone() }
            }),
        );

        let listener = TcpListener::bind(self.port).await.context("bind to port")?;

        tracing::info!(
            name = self.name,
            port = self.port.to_string(),
            "starting service"
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled().await;
            })
            .await
            .context("http failed")?;

        Ok(())
    }
}
