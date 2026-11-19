use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream;
use forage_core::integrations::nats::{
    notification_subject, NotificationEnvelope, STREAM_NAME, STREAM_SUBJECTS,
};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::forest_client::GrpcForestClient;
use crate::notification_worker::proto_to_event;

pub struct NotificationIngester {
    pub grpc: Arc<GrpcForestClient>,
    pub jetstream: jetstream::Context,
    pub service_token: String,
}

impl Component for NotificationIngester {
    fn info(&self) -> ComponentInfo {
        "forage/notification-ingester".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        self.ensure_stream().await.map_err(|e| {
            MadError::Inner(anyhow::anyhow!("failed to create JetStream stream: {e}"))
        })?;

        let mut backoff = 1u64;

        loop {
            let connected_at = Instant::now();
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("notification ingester shutting down");
                    break;
                }
                result = self.ingest_once() => {
                    match result {
                        Ok(()) => {
                            tracing::info!("notification stream ended cleanly");
                            backoff = 1;
                        }
                        Err(e) => {
                            backoff = reconnect_delay_secs(backoff, connected_at.elapsed());
                            if backoff >= MAX_BACKOFF_SECS {
                                tracing::error!(error = %e, backoff_secs = backoff, "notification stream still failing, reconnecting");
                            } else {
                                tracing::warn!(error = %e, backoff_secs = backoff, "notification stream error, reconnecting");
                            }
                        }
                    }

                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF_SECS);
                }
            }
        }

        Ok(())
    }
}

/// Reaching the cap means forest has been unreachable for minutes, not a redeploy.
const MAX_BACKOFF_SECS: u64 = 60;

const STABLE_STREAM: Duration = Duration::from_secs(60);

/// Backoff is for a failure still in progress. A stream that ran for a while and
/// then dropped (every forest-server redeploy does this) reconnects at once, or
/// notifications wait in forest for up to the full backoff.
fn reconnect_delay_secs(backoff_secs: u64, connected_for: Duration) -> u64 {
    if connected_for >= STABLE_STREAM {
        1
    } else {
        backoff_secs
    }
}

#[cfg(test)]
#[path = "tests/notification_ingester_tests.rs"]
mod tests;

impl NotificationIngester {
    async fn ensure_stream(&self) -> Result<(), String> {
        use async_nats::jetstream::stream;

        self.jetstream
            .get_or_create_stream(stream::Config {
                name: STREAM_NAME.to_string(),
                subjects: vec![STREAM_SUBJECTS.to_string()],
                retention: stream::RetentionPolicy::WorkQueue,
                max_age: Duration::from_secs(7 * 24 * 3600),
                max_bytes: 1_073_741_824,
                discard: stream::DiscardPolicy::Old,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("create stream: {e}"))?;

        tracing::info!(stream = STREAM_NAME, "JetStream stream ready");
        Ok(())
    }

    async fn ingest_once(&self) -> Result<(), String> {
        use futures_util::StreamExt;

        let mut client = self.grpc.notification_client();

        let mut req = tonic::Request::new(forage_grpc::ListenNotificationsRequest {
            organisation: None,
            project: None,
        });
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.service_token)
                .parse()
                .map_err(|e| format!("invalid service token: {e}"))?,
        );

        let response = client
            .listen_notifications(req)
            .await
            .map_err(|e| format!("gRPC connect: {e}"))?;

        let mut stream = response.into_inner();

        tracing::info!("connected to notification stream (JetStream mode)");

        while let Some(result) = stream.next().await {
            match result {
                Ok(notification) => {
                    let event = proto_to_event(&notification);
                    tracing::info!(
                        org = %event.organisation,
                        event_type = %event.notification_type,
                        notification_id = %event.id,
                        "received notification, publishing to JetStream"
                    );

                    let envelope = NotificationEnvelope::from(&event);
                    let subject =
                        notification_subject(&event.organisation, &event.notification_type);
                    let payload = serde_json::to_vec(&envelope)
                        .map_err(|e| format!("serialize envelope: {e}"))?;

                    if let Err(e) = self
                        .jetstream
                        .publish(subject, payload.into())
                        .await
                        .map_err(|e| format!("publish: {e}"))
                        .and_then(|ack_future| {
                            tokio::spawn(async move {
                                if let Err(e) = ack_future.await {
                                    tracing::warn!(error = %e, "JetStream publish ack failed");
                                }
                            });
                            Ok(())
                        })
                    {
                        tracing::error!(error = %e, "failed to publish to JetStream");
                    }
                }
                Err(e) => {
                    return Err(format!("stream error: {e}"));
                }
            }
        }

        Ok(())
    }
}
