use std::net::SocketAddr;

use forest_grpc_interface::{
    app_service_server::AppServiceServer,
    artifact_service_server::ArtifactServiceServer,
    auto_release_policy_service_server::AutoReleasePolicyServiceServer,
    destination_service_server::DestinationServiceServer,
    environment_service_server::EnvironmentServiceServer,
    event_service_server::EventServiceServer,
    notification_service_server::NotificationServiceServer,
    organisation_service_server::OrganisationServiceServer,
    registry_service_server::RegistryServiceServer,
    release_pipeline_service_server::ReleasePipelineServiceServer,
    release_service_server::ReleaseServiceServer,
    runner_service_server::RunnerServiceServer,
    status_service_server::StatusServiceServer,
    users_service_server::UsersServiceServer,
};
use notmad::MadError;
use organisations::OrganisationsServer;
use registry::RegistryServer;
use status::StatusServer;
use tokio_util::sync::CancellationToken;

use crate::{
    grpc::{
        artifacts::ArtifactServer, destinations::DestinationServer, release::ReleaseServer,
        users::UsersServer,
    },
    runner_manager::RunnerManager,
    state::State,
};

mod apps;
mod artifacts;
mod auto_release_policies;
mod destinations;
mod events;
mod release_pipelines;
mod environments;
mod error;
mod notifications;
mod organisations;
mod registry;
mod release;
pub mod runner;
mod status;
mod users;

pub struct GrpcServer {
    pub host: SocketAddr,
    pub state: State,
    pub runner_manager: RunnerManager,
}

impl GrpcServer {
    pub async fn serve(&self, cancellation_token: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("serving grpc on {}", self.host);

        let layer = tower::ServiceBuilder::new()
            .layer(log_layer::LogMiddlewareLayer::default())
            .layer(auth_layer::AuthMiddlewareLayer::new(self.state.clone()))
            .into_inner();

        tonic::transport::Server::builder()
            .trace_fn(|_request| tracing::info_span!("grpc"))
            .layer(layer)
            .add_service(StatusServiceServer::new(StatusServer {
                state: self.state.clone(),
            }))
            .add_service(RegistryServiceServer::new(RegistryServer {
                state: self.state.clone(),
            }))
            .add_service(ArtifactServiceServer::new(ArtifactServer {
                state: self.state.clone(),
            }))
            .add_service(ReleaseServiceServer::new(ReleaseServer {
                state: self.state.clone(),
            }))
            .add_service(DestinationServiceServer::new(DestinationServer {
                state: self.state.clone(),
            }))
            .add_service(UsersServiceServer::new(UsersServer {
                state: self.state.clone(),
            }))
            .add_service(OrganisationServiceServer::new(OrganisationsServer {
                state: self.state.clone(),
            }))
            .add_service(AppServiceServer::new(apps::AppsServer {
                state: self.state.clone(),
            }))
            .add_service(EnvironmentServiceServer::new(
                environments::EnvironmentsServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(NotificationServiceServer::new(
                notifications::NotificationsServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(AutoReleasePolicyServiceServer::new(
                auto_release_policies::AutoReleasePoliciesServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(ReleasePipelineServiceServer::new(
                release_pipelines::ReleasePipelinesServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(EventServiceServer::new(events::EventsServer {
                state: self.state.clone(),
            }))
            .add_service(RunnerServiceServer::new(runner::RunnerServer {
                state: self.state.clone(),
                runner_manager: self.runner_manager.clone(),
            }))
            .serve_with_shutdown(
                self.host,
                async move { cancellation_token.cancelled().await },
            )
            .await?;

        Ok(())
    }
}

mod auth_layer;
mod log_layer;

impl notmad::Component for GrpcServer {
    fn info(&self) -> notmad::ComponentInfo {
        "forest-server/grpc".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        self.serve(cancellation_token)
            .await
            .map_err(MadError::Inner)?;

        Ok(())
    }
}
