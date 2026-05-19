use std::sync::Arc;
use std::time::Duration;

use forage_core::integrations::router::{DispatchTask, NotificationEvent, ReleaseContext};
use forage_core::integrations::webhook::sign_payload;
use forage_core::integrations::{DeliveryStatus, IntegrationStore};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::forest_client::GrpcForestClient;

// ── Dispatcher ──────────────────────────────────────────────────────

/// HTTP client for dispatching webhooks and Slack messages.
pub struct NotificationDispatcher {
    http: reqwest::Client,
    store: Arc<dyn IntegrationStore>,
    forage_url: String,
    /// gRPC client for querying pipeline state (optional — absent in tests).
    grpc: Option<Arc<GrpcForestClient>>,
    /// Service token for authenticating gRPC calls to fetch pipeline state.
    service_token: String,
}

impl NotificationDispatcher {
    pub fn new(store: Arc<dyn IntegrationStore>, forage_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");
        Self { http, store, forage_url, grpc: None, service_token: String::new() }
    }

    pub fn with_grpc(mut self, grpc: Arc<GrpcForestClient>, service_token: String) -> Self {
        self.grpc = Some(grpc);
        self.service_token = service_token;
        self
    }

    /// Execute a dispatch task with retry (3 attempts, exponential backoff).
    pub async fn dispatch(&self, task: &DispatchTask) {
        let (integration_id, notification_id) = match task {
            DispatchTask::Webhook {
                integration_id,
                payload,
                ..
            } => (integration_id.clone(), payload.notification_id.clone()),
            DispatchTask::Slack {
                integration_id,
                notification_id,
                ..
            } => (integration_id.clone(), notification_id.clone()),
            DispatchTask::SlackDm {
                integration_id,
                notification_id,
                ..
            } => (integration_id.clone(), notification_id.clone()),
        };

        let delays = [1, 5, 25]; // seconds
        for (attempt, delay) in delays.iter().enumerate() {
            match self.try_dispatch(task).await {
                Ok(()) => {
                    tracing::info!(
                        integration_id = %integration_id,
                        attempt = attempt + 1,
                        "notification delivered"
                    );
                    let _ = self
                        .store
                        .record_delivery(&integration_id, &notification_id, DeliveryStatus::Delivered, None)
                        .await;
                    return;
                }
                Err(e) => {
                    // Don't retry errors that will never succeed
                    let non_retryable = is_non_retryable_error(&e);
                    if non_retryable {
                        tracing::error!(
                            integration_id = %integration_id,
                            error = %e,
                            "non-retryable delivery error"
                        );
                        let _ = self
                            .store
                            .record_delivery(
                                &integration_id,
                                &notification_id,
                                DeliveryStatus::Failed,
                                Some(&e),
                            )
                            .await;
                        return;
                    }

                    tracing::warn!(
                        integration_id = %integration_id,
                        attempt = attempt + 1,
                        error = %e,
                        "delivery attempt failed"
                    );
                    if attempt < delays.len() - 1 {
                        tokio::time::sleep(Duration::from_secs(*delay)).await;
                    } else {
                        tracing::error!(
                            integration_id = %integration_id,
                            "all delivery attempts exhausted"
                        );
                        let _ = self
                            .store
                            .record_delivery(
                                &integration_id,
                                &notification_id,
                                DeliveryStatus::Failed,
                                Some(&e),
                            )
                            .await;
                    }
                }
            }
        }
    }

    async fn try_dispatch(&self, task: &DispatchTask) -> Result<(), String> {
        match task {
            DispatchTask::Webhook {
                url,
                secret,
                headers,
                payload,
                ..
            } => {
                let body =
                    serde_json::to_vec(payload).map_err(|e| format!("serialize: {e}"))?;

                let mut req = self
                    .http
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header("User-Agent", "Forage/1.0");

                if let Some(secret) = secret {
                    let sig = sign_payload(&body, secret);
                    req = req.header("X-Forage-Signature", sig);
                }

                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }

                let resp = req
                    .body(body)
                    .send()
                    .await
                    .map_err(|e| format!("http: {e}"))?;

                let status = resp.status();
                if status.is_success() {
                    Ok(())
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("HTTP {status}: {body}"))
                }
            }
            DispatchTask::Slack {
                integration_id,
                webhook_url,
                access_token,
                channel_id,
                release_id,
                event_type,
                event,
                message,
                ..
            } => {
                // If we have a bot token, use chat.postMessage/chat.update for update-in-place
                if !access_token.is_empty() && !channel_id.is_empty() && !release_id.is_empty() {
                    self.dispatch_slack_bot(
                        integration_id,
                        access_token,
                        channel_id,
                        release_id,
                        event_type,
                        event,
                    )
                    .await
                } else {
                    // Fallback: webhook URL (no update-in-place possible)
                    self.dispatch_slack_webhook(webhook_url, message).await
                }
            }
            DispatchTask::SlackDm {
                integration_id,
                access_token,
                slack_user_id,
                release_id,
                event_type,
                event,
                message: _,
                ..
            } => {
                // DM uses the same bot token post/update pattern, but channel = user ID.
                // Prefix release_id so the message ref is distinct from channel messages.
                let dm_release_id = format!("dm:{slack_user_id}:{release_id}");
                self.dispatch_slack_bot(
                    integration_id,
                    access_token,
                    slack_user_id, // Slack accepts user ID as channel for DMs
                    &dm_release_id,
                    event_type,
                    event,
                )
                .await
            }
        }
    }

    /// Post or update a Slack message via the bot token API.
    /// Merges per-destination status into the message ref and rebuilds the message.
    async fn dispatch_slack_bot(
        &self,
        integration_id: &str,
        access_token: &str,
        channel: &str,
        release_id: &str,
        event_type: &str,
        event: &forage_core::integrations::router::NotificationEvent,
    ) -> Result<(), String> {
        use forage_core::integrations::{DestinationStatus, SlackMessageRef};
        use forage_core::integrations::router::format_slack_message;

        // Get existing ref (with accumulated destinations) if we already posted
        let existing_ref = self
            .store
            .get_slack_message_ref(integration_id, release_id)
            .await
            .unwrap_or(None);

        // Merge this notification's destination into the accumulated map
        let mut destinations = existing_ref
            .as_ref()
            .map(|r| r.destinations.clone())
            .unwrap_or_default();

        if let Some(ref r) = event.release {
            if !r.destination.is_empty() {
                let status = match event_type {
                    "release_started" => "started",
                    "release_succeeded" => "succeeded",
                    "release_failed" => "failed",
                    _ => "started",
                };
                destinations.insert(
                    r.destination.clone(),
                    DestinationStatus {
                        environment: r.environment.clone(),
                        status: status.to_string(),
                        error: r.error_message.clone(),
                    },
                );
            }
        }

        // Build the message with the full accumulated state
        let mut message = format_slack_message(event, &destinations, &self.forage_url);

        // Query pipeline stages and insert before destinations
        if let Some(ref r) = event.release {
            if !r.release_intent_id.is_empty() {
                if let Some(stages) = self
                    .fetch_pipeline_stages(&event.organisation, &event.project, &r.release_intent_id)
                    .await
                {
                    let pipeline_blocks =
                        forage_core::integrations::router::format_pipeline_blocks(&stages);
                    if !pipeline_blocks.is_empty() {
                        // Insert pipeline before the destination section.
                        // Find the last "context" block (metadata); pipeline goes right after it,
                        // pushing destinations and errors down.
                        let insert_at = message
                            .blocks
                            .iter()
                            .rposition(|b| b["type"] == "context")
                            .map(|i| i + 1)
                            .unwrap_or(message.blocks.len());
                        for (i, block) in pipeline_blocks.into_iter().enumerate() {
                            message.blocks.insert(insert_at + i, block);
                        }
                    }
                }
            }
        }
        let release_title = event
            .release
            .as_ref()
            .filter(|r| !r.context_title.is_empty())
            .map(|r| r.context_title.clone())
            .or_else(|| existing_ref.as_ref().map(|r| r.release_title.clone()))
            .unwrap_or_else(|| event.title.clone());

        let blocks_payload = serde_json::json!([{
            "color": message.color,
            "blocks": message.blocks,
        }]);

        // The `text` field is a fallback for notifications/accessibility only.
        // Slack renders it above attachments in some clients, causing duplication.
        // Use a minimal fallback; the attachment blocks carry the rich content.
        let fallback_text = format!("Release update: {}/{}", event.organisation, event.project);

        if let Some(ref msg_ref) = existing_ref {
            // Update existing message
            let payload = serde_json::json!({
                "channel": msg_ref.channel_id,
                "ts": msg_ref.message_ts,
                "text": fallback_text,
                "attachments": blocks_payload,
            });

            let resp = self
                .http
                .post("https://slack.com/api/chat.update")
                .bearer_auth(access_token)
                .json(&payload)
                .send()
                .await
                .map_err(|e| format!("slack chat.update http: {e}"))?;

            let body: serde_json::Value =
                resp.json().await.map_err(|e| format!("slack chat.update parse: {e}"))?;

            if body["ok"].as_bool() != Some(true) {
                let err = body["error"].as_str().unwrap_or("unknown");
                if err == "message_not_found" {
                    tracing::warn!(
                        integration_id = %integration_id,
                        release_id = %release_id,
                        "slack message not found, posting new one"
                    );
                    // Fall through to post a new one
                } else {
                    return Err(format!("slack chat.update: {err}"));
                }
            } else {
                // Update the ref with merged destinations
                let updated = SlackMessageRef {
                    id: msg_ref.id.clone(),
                    integration_id: integration_id.to_string(),
                    release_id: release_id.to_string(),
                    channel_id: msg_ref.channel_id.clone(),
                    message_ts: msg_ref.message_ts.clone(),
                    last_event_type: event_type.to_string(),
                    destinations,
                    release_title,
                    created_at: msg_ref.created_at.clone(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                };
                let _ = self.store.upsert_slack_message_ref(&updated).await;
                return Ok(());
            }
        }

        // Try to join the channel first
        let _ = self.slack_join_channel(access_token, channel).await;

        // Post new message
        let payload = serde_json::json!({
            "channel": channel,
            "text": fallback_text,
            "attachments": blocks_payload,
        });

        let resp = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("slack chat.postMessage http: {e}"))?;

        let body: serde_json::Value =
            resp.json().await.map_err(|e| format!("slack chat.postMessage parse: {e}"))?;

        if body["ok"].as_bool() != Some(true) {
            let err = body["error"].as_str().unwrap_or("unknown");
            return Err(format!("slack chat.postMessage: {err}"));
        }

        // Store the message ref with initial destinations
        let ts = body["ts"].as_str().unwrap_or_default();
        let posted_channel = body["channel"].as_str().unwrap_or(channel);

        if !ts.is_empty() && !release_id.is_empty() {
            let msg_ref = SlackMessageRef {
                id: uuid::Uuid::new_v4().to_string(),
                integration_id: integration_id.to_string(),
                release_id: release_id.to_string(),
                channel_id: posted_channel.to_string(),
                message_ts: ts.to_string(),
                last_event_type: event_type.to_string(),
                destinations,
                release_title,
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            let _ = self.store.upsert_slack_message_ref(&msg_ref).await;
        }

        Ok(())
    }

    /// Try to join a Slack channel. Silently succeeds if already a member or channel is private.
    async fn slack_join_channel(&self, access_token: &str, channel: &str) -> Result<(), String> {
        let payload = serde_json::json!({ "channel": channel });

        let resp = self
            .http
            .post("https://slack.com/api/conversations.join")
            .bearer_auth(access_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("slack conversations.join http: {e}"))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("slack conversations.join parse: {e}"))?;

        if body["ok"].as_bool() == Some(true) {
            tracing::info!(channel = %channel, "bot joined slack channel");
        } else {
            let err = body["error"].as_str().unwrap_or("unknown");
            // channel_not_found, method_not_supported_for_channel_type (private), already_in_channel
            // These are all acceptable — we tried our best
            tracing::debug!(channel = %channel, error = %err, "conversations.join failed (may be private channel)");
        }

        Ok(())
    }

    /// Fetch pipeline stages for a release intent via gRPC.
    /// Returns None if gRPC is not configured or the call fails.
    async fn fetch_pipeline_stages(
        &self,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
    ) -> Option<Vec<forage_core::platform::PipelineRunStageState>> {
        let grpc = self.grpc.as_ref()?;
        if self.service_token.is_empty() {
            return None;
        }

        match grpc
            .get_release_intent_states_with_token(
                &self.service_token,
                organisation,
                Some(project),
                true, // include_completed so we get the current intent
            )
            .await
        {
            Ok(intents) => {
                // Find the matching release intent
                intents
                    .into_iter()
                    .find(|i| i.release_intent_id == release_intent_id)
                    .map(|i| i.stages)
            }
            Err(e) => {
                tracing::warn!(
                    release_intent_id = %release_intent_id,
                    error = %e,
                    "failed to fetch pipeline stages"
                );
                None
            }
        }
    }

    /// Fallback: post via incoming webhook URL (no update-in-place).
    async fn dispatch_slack_webhook(
        &self,
        webhook_url: &str,
        message: &forage_core::integrations::router::SlackMessage,
    ) -> Result<(), String> {
        let payload = serde_json::json!({
            "text": message.text,
            "attachments": [{
                "color": message.color,
                "blocks": message.blocks,
            }]
        });

        let resp = self
            .http
            .post(webhook_url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("slack http: {e}"))?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(format!("Slack HTTP {status}: {body}"))
        }
    }
}

/// Slack API errors that will never succeed on retry.
fn is_non_retryable_error(err: &str) -> bool {
    const NON_RETRYABLE: &[&str] = &[
        "channel_not_found",
        "not_in_channel",
        "is_archived",
        "invalid_auth",
        "token_revoked",
        "account_inactive",
        "no_permission",
        "missing_scope",
        "not_authed",
        "invalid_arguments",
    ];
    NON_RETRYABLE.iter().any(|code| err.contains(code))
}

// ── Proto conversion ────────────────────────────────────────────────

/// Convert a proto Notification to our domain NotificationEvent.
pub fn proto_to_event(n: &forage_grpc::Notification) -> NotificationEvent {
    let notification_type = match n.notification_type() {
        forage_grpc::NotificationType::ReleaseAnnotated => "release_annotated",
        forage_grpc::NotificationType::ReleaseStarted => "release_started",
        forage_grpc::NotificationType::ReleaseSucceeded => "release_succeeded",
        forage_grpc::NotificationType::ReleaseFailed => "release_failed",
        _ => "unknown",
    };

    let release = n.release_context.as_ref().map(|r| ReleaseContext {
        slug: r.slug.clone(),
        artifact_id: r.artifact_id.clone(),
        release_intent_id: r.release_intent_id.clone(),
        destination: r.destination.clone(),
        environment: r.environment.clone(),
        source_username: r.source_username.clone(),
        source_user_id: r.source_user_id.clone(),
        commit_sha: r.commit_sha.clone(),
        commit_branch: r.commit_branch.clone(),
        context_title: r.context_title.clone(),
        context_web: r.context_web.clone(),
        destination_count: r.destination_count,
        error_message: if r.error_message.is_empty() {
            None
        } else {
            Some(r.error_message.clone())
        },
    });

    NotificationEvent {
        id: n.id.clone(),
        notification_type: notification_type.to_string(),
        title: n.title.clone(),
        body: n.body.clone(),
        organisation: n.organisation.clone(),
        project: n.project.clone(),
        timestamp: n.created_at.clone(),
        release,
    }
}

// ── Listener component ──────────────────────────────────────────────

/// Background component that listens to Forest's notification stream
/// for all orgs with active integrations, and dispatches to configured channels.
pub struct NotificationListener {
    pub grpc: Arc<GrpcForestClient>,
    pub store: Arc<dyn IntegrationStore>,
    /// Service token (PAT) for authenticating with forest-server's NotificationService.
    pub service_token: String,
    /// Base URL of the Forage web UI for deep links (e.g. "https://forage.example.com").
    pub forage_url: String,
}

impl Component for NotificationListener {
    fn info(&self) -> ComponentInfo {
        "forage/notification-listener".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let dispatcher = Arc::new(
            NotificationDispatcher::new(self.store.clone(), self.forage_url.clone())
                .with_grpc(self.grpc.clone(), self.service_token.clone()),
        );

        // For now, listen on the global stream (no org filter).
        // Forest's ListenNotifications with no org filter returns all notifications
        // the authenticated user has access to.
        let mut backoff = 1u64;

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("notification listener shutting down");
                    break;
                }
                result = self.listen_once(&dispatcher) => {
                    match result {
                        Ok(()) => {
                            tracing::info!("notification stream ended cleanly");
                            backoff = 1;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, backoff_secs = backoff, "notification stream error, reconnecting");
                        }
                    }

                    // Wait before reconnecting, but respect cancellation
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

impl NotificationListener {
    async fn listen_once(&self, dispatcher: &Arc<NotificationDispatcher>) -> Result<(), String> {
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

        tracing::info!("connected to notification stream");

        while let Some(result) = stream.next().await {
            match result {
                Ok(notification) => {
                    let event = proto_to_event(&notification);
                    tracing::info!(
                        org = %event.organisation,
                        event_type = %event.notification_type,
                        notification_id = %event.id,
                        "received notification"
                    );

                    let tasks = forage_core::integrations::router::route_notification_for_org(
                        self.store.as_ref(),
                        &event,
                    )
                    .await;

                    for task in &tasks {
                        let dispatcher = dispatcher.clone();
                        let task = task.clone();
                        tokio::spawn(async move {
                            dispatcher.dispatch(&task).await;
                        });
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
