use forest_grpc_interface::{
    release_health_service_server::ReleaseHealthService, DestinationHealth, GetReleaseHealthRequest,
    GetReleaseHealthResponse, HealthStatus, ReleaseHealthEvent,
    ReportHealthRequest, ReportHealthResponse, WatchReleaseHealthRequest,
};
use futures::StreamExt;
use uuid::Uuid;

use crate::grpc::authorize;
use crate::services::release_health;
use crate::state::State;

pub struct ReleaseHealthServer {
    pub state: State,
}

fn status_string_to_proto(s: &str) -> i32 {
    match s {
        "HEALTHY" => HealthStatus::Healthy as i32,
        "PROGRESSING" => HealthStatus::Progressing as i32,
        "DEGRADED" => HealthStatus::Degraded as i32,
        "UNHEALTHY" => HealthStatus::Unhealthy as i32,
        "MISSING" => HealthStatus::Missing as i32,
        _ => HealthStatus::Unspecified as i32,
    }
}

fn proto_status_to_string(status: i32) -> &'static str {
    match HealthStatus::try_from(status) {
        Ok(HealthStatus::Healthy) => "HEALTHY",
        Ok(HealthStatus::Progressing) => "PROGRESSING",
        Ok(HealthStatus::Degraded) => "DEGRADED",
        Ok(HealthStatus::Unhealthy) => "UNHEALTHY",
        Ok(HealthStatus::Missing) => "MISSING",
        _ => "UNSPECIFIED",
    }
}

#[async_trait::async_trait]
impl ReleaseHealthService for ReleaseHealthServer {
    async fn report_health(
        &self,
        request: tonic::Request<ReportHealthRequest>,
    ) -> Result<tonic::Response<ReportHealthResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let release_intent_id = Uuid::parse_str(&req.release_intent_id)
            .map_err(|e| tonic::Status::invalid_argument(format!("invalid release_intent_id: {e}")))?;

        let release_id = Uuid::parse_str(&req.release_id)
            .map_err(|e| tonic::Status::invalid_argument(format!("invalid release_id: {e}")))?;

        let observation = req
            .observation
            .ok_or_else(|| tonic::Status::invalid_argument("observation is required"))?;

        // Serialize proto to JSON via prost encoding → base64 → JSON wrapper.
        // We store the observation as a structured JSON document.
        let observation_json = {
            let resources: Vec<serde_json::Value> = observation
                .resources
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "kind": r.kind,
                        "name": r.name,
                        "namespace": r.namespace,
                        "status": proto_status_to_string(r.status),
                        "message": r.message,
                        "properties": r.properties,
                    })
                })
                .collect();

            serde_json::json!({
                "resources": resources,
                "observed_at": observation.observed_at,
                "status": proto_status_to_string(observation.status),
                "message": observation.message,
            })
        };

        let status = proto_status_to_string(observation.status);
        let observed_at = chrono::DateTime::parse_from_rfc3339(&observation.observed_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        release_health::upsert_observation(
            &self.state.db,
            &self.state.nats,
            release_intent_id,
            release_id,
            &req.destination,
            &req.environment,
            &req.organisation,
            &req.project,
            &observation_json,
            status,
            &observation.message,
            &observed_at,
        )
        .await
        .map_err(|e| tonic::Status::internal(format!("upsert observation: {e}")))?;

        tracing::debug!(
            project = req.project,
            destination = req.destination,
            status = status,
            "health observation recorded"
        );

        Ok(tonic::Response::new(ReportHealthResponse {}))
    }

    async fn get_release_health(
        &self,
        request: tonic::Request<GetReleaseHealthRequest>,
    ) -> Result<tonic::Response<GetReleaseHealthResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let release_intent_id = Uuid::parse_str(&req.release_intent_id)
            .map_err(|e| tonic::Status::invalid_argument(format!("invalid release_intent_id: {e}")))?;

        authorize_intent(&self.state.db, &actor, release_intent_id).await?;

        let rows = release_health::get_observations_for_intent(&self.state.db, release_intent_id)
            .await
            .map_err(|e| tonic::Status::internal(format!("get observations: {e}")))?;

        let aggregate = release_health::aggregate_status(&rows);

        let destinations = rows
            .into_iter()
            .map(|row| {
                DestinationHealth {
                    destination: row.destination_name,
                    environment: row.environment,
                    latest_observation: None, // Full observation is in DB as JSON; proto reconstruction TODO
                    status: status_string_to_proto(&row.status),
                }
            })
            .collect();

        Ok(tonic::Response::new(GetReleaseHealthResponse {
            destinations,
            aggregate_status: status_string_to_proto(aggregate),
        }))
    }

    type WatchReleaseHealthStream = tokio_stream::wrappers::ReceiverStream<
        Result<ReleaseHealthEvent, tonic::Status>,
    >;

    async fn watch_release_health(
        &self,
        request: tonic::Request<WatchReleaseHealthRequest>,
    ) -> Result<tonic::Response<Self::WatchReleaseHealthStream>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let release_intent_id = Uuid::parse_str(&req.release_intent_id)
            .map_err(|e| tonic::Status::invalid_argument(format!("invalid release_intent_id: {e}")))?;

        authorize_intent(&self.state.db, &actor, release_intent_id).await?;

        let nats = self.state.nats.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            let nats_subject = format!("forest.release.health.{}", release_intent_id);
            let mut sub = match nats.subscribe(nats_subject).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "failed to subscribe to health events");
                    return;
                }
            };

            while let Some(msg) = sub.next().await {
                let payload: serde_json::Value = match serde_json::from_slice(&msg.payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let event = ReleaseHealthEvent {
                    destination: payload["destination"].as_str().unwrap_or("").to_string(),
                    environment: payload["environment"].as_str().unwrap_or("").to_string(),
                    observation: None, // Lightweight event — client can call GetReleaseHealth for full data
                    status: status_string_to_proto(
                        payload["status"].as_str().unwrap_or("UNSPECIFIED"),
                    ),
                };

                if tx.send(Ok(event)).await.is_err() {
                    break; // Client disconnected
                }
            }
        });

        Ok(tonic::Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

/// Resolve a release intent's owning organisation and check membership.
/// Returns NotFound if the intent doesn't exist (avoiding a probing
/// oracle for unauthenticated callers).
async fn authorize_intent(
    db: &sqlx::PgPool,
    actor: &crate::actor::Actor,
    release_intent_id: Uuid,
) -> Result<(), tonic::Status> {
    let org = sqlx::query_scalar!(
        "SELECT p.organisation FROM release_intents ri
         JOIN projects p ON p.id = ri.project_id
         WHERE ri.id = $1",
        release_intent_id,
    )
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!("authz: resolve intent org failed: {e}");
        tonic::Status::internal("authorization lookup failed")
    })?
    .ok_or_else(|| tonic::Status::not_found("release intent not found"))?;

    authorize::require_org_access(db, actor, &org, authorize::OrgRole::Member).await?;
    Ok(())
}
