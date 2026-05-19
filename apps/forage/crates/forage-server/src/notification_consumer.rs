use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::PullConsumer;
use forage_core::integrations::nats::{
    NotificationEnvelope, CONSUMER_NAME, STREAM_NAME,
};
use forage_core::integrations::IntegrationStore;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::forest_client::GrpcForestClient;
use crate::notification_worker::NotificationDispatcher;

/// Background component that pulls notification events from NATS JetStream
/// and dispatches webhooks to matching integrations.
pub struct NotificationConsumer {
    pub jetstream: jetstream::Context,
    pub store: Arc<dyn IntegrationStore>,
    pub forage_url: String,
    pub grpc: Arc<GrpcForestClient>,
    pub service_token: String,
}

impl Component for NotificationConsumer {
    fn info(&self) -> ComponentInfo {
        "forage/notification-consumer".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let dispatcher = Arc::new(
            NotificationDispatcher::new(self.store.clone(), self.forage_url.clone())
                .with_grpc(self.grpc.clone(), self.service_token.clone()),
        );

        let mut backoff = 1u64;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("notification consumer shutting down");
                    break;
                }
                result = self.consume_loop(&dispatcher, &cancellation_token) => {
                    match result {
                        Ok(()) => {
                            tracing::info!("consumer loop ended cleanly");
                            backoff = 1;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, backoff_secs = backoff, "consumer error, reconnecting");
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

impl NotificationConsumer {
    async fn get_or_create_consumer(&self) -> Result<PullConsumer, String> {
        use async_nats::jetstream::consumer;

        let stream = self
            .jetstream
            .get_stream(STREAM_NAME)
            .await
            .map_err(|e| format!("get stream: {e}"))?;

        stream
            .get_or_create_consumer(
                CONSUMER_NAME,
                consumer::pull::Config {
                    durable_name: Some(CONSUMER_NAME.to_string()),
                    ack_wait: Duration::from_secs(120),
                    max_deliver: 5,
                    max_ack_pending: 100,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| format!("create consumer: {e}"))
    }

    async fn consume_loop(
        &self,
        dispatcher: &Arc<NotificationDispatcher>,
        cancellation_token: &CancellationToken,
    ) -> Result<(), String> {
        use futures_util::StreamExt;

        let consumer = self.get_or_create_consumer().await?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| format!("consumer messages: {e}"))?;

        tracing::info!(consumer = CONSUMER_NAME, "pulling from JetStream");

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    return Ok(());
                }
                msg = messages.next() => {
                    let Some(msg) = msg else {
                        return Ok(()); // Stream closed
                    };
                    let msg = msg.map_err(|e| format!("message error: {e}"))?;

                    match self.handle_message(&msg, dispatcher).await {
                        Ok(()) => {
                            if let Err(e) = msg.ack().await {
                                tracing::warn!(error = %e, "failed to ack message");
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "failed to handle message, nacking");
                            if let Err(e) = msg.ack_with(async_nats::jetstream::AckKind::Nak(Some(Duration::from_secs(30)))).await {
                                tracing::warn!(error = %e, "failed to nak message");
                            }
                        }
                    }
                }
            }
        }
    }

    async fn handle_message(
        &self,
        msg: &async_nats::jetstream::Message,
        dispatcher: &Arc<NotificationDispatcher>,
    ) -> Result<(), String> {
        Self::process_payload(&msg.payload, self.store.as_ref(), dispatcher).await
    }

    /// Process a raw notification payload. Extracted for testability without NATS.
    pub async fn process_payload(
        payload: &[u8],
        store: &dyn IntegrationStore,
        dispatcher: &NotificationDispatcher,
    ) -> Result<(), String> {
        let envelope: NotificationEnvelope = serde_json::from_slice(payload)
            .map_err(|e| format!("deserialize envelope: {e}"))?;

        let event: forage_core::integrations::router::NotificationEvent = envelope.into();

        tracing::info!(
            org = %event.organisation,
            event_type = %event.notification_type,
            notification_id = %event.id,
            "processing notification from JetStream"
        );

        let tasks = forage_core::integrations::router::route_notification_for_org(
            store,
            &event,
        )
        .await;

        if tasks.is_empty() {
            tracing::debug!(
                org = %event.organisation,
                "no matching integrations, skipping"
            );
            return Ok(());
        }

        // Dispatch all tasks sequentially within this message.
        // JetStream provides parallelism across messages.
        for task in &tasks {
            dispatcher.dispatch(task).await;
        }

        Ok(())
    }
}
