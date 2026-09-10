//! The `SignalService` RPCs. See `interface/proto/forest/v1/signals.proto`
//! for what a signal is and why reporting is open to more than the deploying
//! provider.

use forest_grpc_interface::{
    HealthStatus, ListSignalsRequest, ListSignalsResponse, ReportSignalRequest,
    ReportSignalResponse, Signal, signal_service_server::SignalService,
};
use uuid::Uuid;

use crate::grpc::authorize;
use crate::services::release_signals::{self, SignalRow};
use crate::state::State;

pub struct SignalServer {
    pub state: State,
}

/// `HealthStatus` is reused as the signal vocabulary, so this is the only
/// place the wire enum and the stored string meet.
fn proto_status_to_string(status: i32) -> Option<&'static str> {
    match HealthStatus::try_from(status).ok()? {
        HealthStatus::Healthy => Some("HEALTHY"),
        HealthStatus::Progressing => Some("PROGRESSING"),
        HealthStatus::Degraded => Some("DEGRADED"),
        HealthStatus::Unhealthy => Some("UNHEALTHY"),
        HealthStatus::Missing => Some("MISSING"),
        // Rejected rather than stored. A signal with no status cannot satisfy
        // a gate and cannot fail one, so accepting it would mean a reporter
        // believing it had reported while nothing could ever act on it.
        HealthStatus::Unspecified => None,
    }
}

fn status_string_to_proto(status: &str) -> i32 {
    match status {
        "HEALTHY" => HealthStatus::Healthy as i32,
        "PROGRESSING" => HealthStatus::Progressing as i32,
        "DEGRADED" => HealthStatus::Degraded as i32,
        "UNHEALTHY" => HealthStatus::Unhealthy as i32,
        "MISSING" => HealthStatus::Missing as i32,
        _ => HealthStatus::Unspecified as i32,
    }
}

#[tonic::async_trait]
impl SignalService for SignalServer {
    async fn report_signal(
        &self,
        request: tonic::Request<ReportSignalRequest>,
    ) -> Result<tonic::Response<ReportSignalResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let release_intent_id = Uuid::parse_str(&req.release_intent_id).map_err(|e| {
            tonic::Status::invalid_argument(format!("invalid release_intent_id: {e}"))
        })?;
        let release_id = Uuid::parse_str(&req.release_id)
            .map_err(|e| tonic::Status::invalid_argument(format!("invalid release_id: {e}")))?;

        let signal = req
            .signal
            .ok_or_else(|| tonic::Status::invalid_argument("signal is required"))?;

        if signal.name.trim().is_empty() {
            return Err(tonic::Status::invalid_argument(
                "signal.name is required — a gate waits on a signal by name",
            ));
        }
        if signal.destination.trim().is_empty() {
            return Err(tonic::Status::invalid_argument(
                "signal.destination is required — a signal is about a release on one destination",
            ));
        }

        let status = proto_status_to_string(signal.status).ok_or_else(|| {
            tonic::Status::invalid_argument(format!(
                "signal.status must be one of {:?}, got {}",
                release_signals::VALID_STATUSES,
                signal.status,
            ))
        })?;

        // An unparseable timestamp becomes "now" rather than an error, but it
        // is worth a warning: a gate compares `observed_at` against the stage
        // before it, so a reporter with a broken clock format silently gets
        // its observation dated on arrival instead.
        let observed_at = match chrono::DateTime::parse_from_rfc3339(&signal.observed_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(e) => {
                tracing::warn!(
                    observed_at = signal.observed_at,
                    reported_by = signal.reported_by,
                    "signal.observed_at is not RFC 3339 ({e}); dating it on arrival"
                );
                chrono::Utc::now()
            }
        };

        let row = SignalRow {
            name: signal.name,
            status: status.to_string(),
            detail: signal.detail,
            destination_name: signal.destination,
            environment: signal.environment,
            reported_by: signal.reported_by,
            observed_at,
        };

        release_signals::report(
            &self.state.db,
            &self.state.nats,
            release_intent_id,
            release_id,
            &req.organisation,
            &req.project,
            &row,
        )
        .await
        .map_err(|e| tonic::Status::internal(format!("report signal: {e}")))?;

        tracing::debug!(
            project = req.project,
            destination = row.destination_name,
            signal = row.name,
            status = row.status,
            "release signal recorded"
        );

        Ok(tonic::Response::new(ReportSignalResponse {}))
    }

    async fn list_signals(
        &self,
        request: tonic::Request<ListSignalsRequest>,
    ) -> Result<tonic::Response<ListSignalsResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let release_intent_id = Uuid::parse_str(&req.release_intent_id).map_err(|e| {
            tonic::Status::invalid_argument(format!("invalid release_intent_id: {e}"))
        })?;

        authorize::require_intent_access(&self.state.db, &actor, release_intent_id).await?;

        let rows = release_signals::list_for_intent(&self.state.db, release_intent_id)
            .await
            .map_err(|e| tonic::Status::internal(format!("list signals: {e}")))?;

        let signals = rows
            .into_iter()
            .map(|r| Signal {
                status: status_string_to_proto(&r.status),
                name: r.name,
                detail: r.detail,
                destination: r.destination_name,
                environment: r.environment,
                observed_at: r.observed_at.to_rfc3339(),
                reported_by: r.reported_by,
            })
            .collect();

        Ok(tonic::Response::new(ListSignalsResponse { signals }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_round_trips_through_the_wire_and_back() {
        for status in release_signals::VALID_STATUSES {
            let wire = status_string_to_proto(status);
            assert_eq!(
                proto_status_to_string(wire),
                Some(status),
                "{status} should survive the round trip"
            );
        }
    }

    /// UNSPECIFIED is the default a client gets by simply not setting the
    /// field, which is exactly why it must not be storable: it would look like
    /// a report and satisfy nothing.
    #[test]
    fn unspecified_is_not_a_reportable_status() {
        assert_eq!(
            proto_status_to_string(HealthStatus::Unspecified as i32),
            None
        );
        assert_eq!(proto_status_to_string(0), None);
    }

    #[test]
    fn an_unknown_wire_value_is_not_a_reportable_status() {
        assert_eq!(proto_status_to_string(99), None);
        assert_eq!(proto_status_to_string(-1), None);
    }
}
