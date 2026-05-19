use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use forage_core::integrations::nats::{
    notification_subject, NotificationEnvelope, STREAM_NAME, STREAM_SUBJECTS,
};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::forest_client::GrpcForestClient;
use crate::notification_worker::proto_to_event;

/// Background component that listens to Forest's notification stream
/// and publishes events to NATS JetStream for durable processing.
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
        // Ensure the JetStream stream exists
        self.ensure_stream().await.map_err(|e| {
            MadError::Inner(anyhow::anyhow!("failed to create JetStream stream: {e}"))
        })?;

        let mut backoff = 1u64;

        loop {
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
                            tracing::error!(error = %e, backoff_secs = backoff, "notification stream error, reconnecting");
                        }
                    }

                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    }
                    backoff = (backoff * 2).min(60);
                }
            }
        }

        Ok(())
    }
}

impl NotificationIngester {
    async fn ensure_stream(&self) -> Result<(), String> {
        use async_nats::jetstream::stream;

        self.jetstream
            .get_or_create_stream(stream::Config {
                name: STREAM_NAME.to_string(),
                subjects: vec![STREAM_SUBJECTS.to_string()],
                retention: stream::RetentionPolicy::WorkQueue,
                max_age: Duration::from_secs(7 * 24 * 3600), // 7 days
                max_bytes: 1_073_741_824,                     // 1 GB
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

                    // Publish with ack — JetStream confirms persistence
                    if let Err(e) = self
                        .jetstream
                        .publish(subject, payload.into())
                        .await
                        .map_err(|e| format!("publish: {e}"))
                        .and_then(|ack_future| {
                            // We don't block on the ack to keep the stream flowing,
                            // but we log failures. In practice, JetStream will buffer.
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
