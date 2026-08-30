use std::net::SocketAddr;

use forest_grpc_interface::{
    app_service_server::AppServiceServer, artifact_service_server::ArtifactServiceServer,
    destination_service_server::DestinationServiceServer,
    environment_service_server::EnvironmentServiceServer, event_service_server::EventServiceServer,
    event_subscription_service_server::EventSubscriptionServiceServer,
    notification_service_server::NotificationServiceServer,
    o_auth_apps_service_server::OAuthAppsServiceServer,
    org_rule_set_service_server::OrgRuleSetServiceServer,
    organisation_service_server::OrganisationServiceServer,
    policy_service_server::PolicyServiceServer, registry_service_server::RegistryServiceServer,
    release_health_service_server::ReleaseHealthServiceServer,
    release_pipeline_service_server::ReleasePipelineServiceServer,
    release_service_server::ReleaseServiceServer, runner_service_server::RunnerServiceServer,
    status_service_server::StatusServiceServer, trigger_service_server::TriggerServiceServer,
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
pub(crate) mod authorize;
mod destinations;
mod environments;
mod error;
mod event_subscriptions;
mod events;
mod notifications;
mod oauth_apps;
mod org_rules;
mod organisations;
mod policies;
mod registry;
mod release;
mod release_health;
mod release_pipelines;
pub mod runner;
mod status;
mod triggers;
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

        // Standard gRPC health protocol, for the ALB target group fronting 4040.
        //
        // Until this existed the probe hit an unregistered method and got back
        // UNIMPLEMENTED, which the target group's matcher was widened to `0-99`
        // to tolerate — so the check passed on any response at all and proved
        // nothing beyond "the HTTP/2 stack answers". With a real service
        // registered, a passing check means the server accepted a request and
        // routed it through tonic end to end.
        //
        // Reports SERVING unconditionally, which makes this a liveness signal
        // rather than a readiness one. That is deliberate: forest's readiness
        // aggregates Aurora reachability, and wiring it in here would let one
        // database blip pull every forest target out of the load balancer
        // simultaneously — turning a degraded service into a total outage.
        // Readiness belongs on the HTTP target group, which already probes it.
        let (_health_reporter, health_service) = tonic_health::server::health_reporter();

        tonic::transport::Server::builder()
            .trace_fn(|_request| tracing::info_span!("grpc"))
            .layer(layer)
            .add_service(health_service)
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
            .add_service(OAuthAppsServiceServer::new(oauth_apps::OAuthAppsServer {
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
            .add_service(TriggerServiceServer::new(triggers::TriggersServer {
                state: self.state.clone(),
            }))
            .add_service(PolicyServiceServer::new(policies::PoliciesServer {
                state: self.state.clone(),
            }))
            .add_service(ReleasePipelineServiceServer::new(
                release_pipelines::ReleasePipelinesServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(OrgRuleSetServiceServer::new(org_rules::OrgRulesServer {
                state: self.state.clone(),
            }))
            .add_service(EventServiceServer::new(events::EventsServer {
                state: self.state.clone(),
            }))
            .add_service(EventSubscriptionServiceServer::new(
                event_subscriptions::EventSubscriptionsServer {
                    state: self.state.clone(),
                },
            ))
            .add_service(RunnerServiceServer::new(runner::RunnerServer {
                state: self.state.clone(),
                runner_manager: self.runner_manager.clone(),
            }))
            .add_service(ReleaseHealthServiceServer::new(
                release_health::ReleaseHealthServer {
                    state: self.state.clone(),
                },
            ))
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
