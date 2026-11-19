use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use forage_core::integrations::router::{DispatchTask, NotificationEvent, ReleaseContext};
use forage_core::integrations::webhook::sign_payload;
use forage_core::integrations::slack_release::{self, ReleaseView, ThreadDetail};
use forage_core::integrations::{
    DeliveryStatus, DestinationStatus, IntegrationStore, SlackMessageRef,
};
use forage_core::platform::{
    Artifact, ForestPlatform, PipelineRunStageState, PlanOutput, ReleaseStepState,
};
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::forest_client::GrpcForestClient;

pub struct NotificationDispatcher {
    http: reqwest::Client,
    store: Arc<dyn IntegrationStore>,
    forage_url: String,
    grpc: Option<Arc<GrpcForestClient>>,
    service_token: String,
    slack: crate::providers::SlackApi,
    details: Option<Arc<dyn ReleaseDetailSource>>,
    slack_permits: tokio::sync::Semaphore,
}

const MAX_RATE_LIMIT_WAIT_SECS: u64 = 60;

/// Each Slack release dispatch holds a pooled Postgres connection for its
/// advisory lock; this keeps them well under the pool's default of 10.
const MAX_CONCURRENT_SLACK_RELEASES: usize = 4;

/// Thread replies rendered under the release lock but posted after it is
/// released: Slack calls can wait on rate limits, and each reply's claim is
/// atomic on its own.
struct PendingThread {
    channel_id: String,
    message_ts: String,
    posts: Vec<slack_release::ThreadPost>,
}

#[derive(Default)]
struct FetchedDetail {
    artifacts: Option<Vec<forage_core::platform::Artifact>>,
    plans: HashMap<String, forage_core::platform::PlanOutput>,
}

impl NotificationDispatcher {
    pub fn new(store: Arc<dyn IntegrationStore>, forage_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");
        Self {
            http,
            store,
            forage_url,
            grpc: None,
            service_token: String::new(),
            slack: crate::providers::SlackApi::default(),
            details: None,
            slack_permits: tokio::sync::Semaphore::new(MAX_CONCURRENT_SLACK_RELEASES),
        }
    }

    pub fn with_slack_api(mut self, slack: crate::providers::SlackApi) -> Self {
        self.slack = slack;
        self
    }

    pub fn with_grpc(mut self, grpc: Arc<GrpcForestClient>, service_token: String) -> Self {
        if !service_token.is_empty() {
            self.details = Some(Arc::new(ForestReleaseDetails {
                grpc: grpc.clone(),
                service_token: service_token.clone(),
            }));
        }
        self.grpc = Some(grpc);
        self.service_token = service_token;
        self
    }

    pub fn with_release_details(mut self, details: Arc<dyn ReleaseDetailSource>) -> Self {
        self.details = Some(details);
        self
    }

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

        let delays = [1, 5, 25];
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
                    let non_retryable = is_non_retryable_error(&e);
                    if non_retryable {
                        tracing::error!(
                            integration_id = %integration_id,
                            error = %e,
                            "non-retryable delivery error"
                        );
                        if is_revoked_token_error(&e) {
                            self.disable_revoked(task, &integration_id).await;
                        }
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
                    .header("User-Agent", "Forest/1.0");

                if let Some(secret) = secret {
                    let sig = sign_payload(&body, secret);
                    req = req.header("X-Forest-Signature", sig);
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
                let dm_release_id = format!("dm:{slack_user_id}:{release_id}");
                self.dispatch_slack_bot(
                    integration_id,
                    access_token,
                    slack_user_id,
                    &dm_release_id,
                    event_type,
                    event,
                )
                .await
            }
        }
    }

    async fn dispatch_slack_bot(
        &self,
        integration_id: &str,
        access_token: &str,
        channel: &str,
        release_id: &str,
        event_type: &str,
        event: &NotificationEvent,
    ) -> Result<(), String> {
        let _permit = self
            .slack_permits
            .acquire()
            .await
            .map_err(|e| format!("slack permits: {e}"))?;
        let lock = self
            .store
            .lock_slack_release(integration_id, release_id)
            .await
            .map_err(|e| format!("lock slack release: {e}"))?;
        let rendered = self
            .render_slack_release(
                integration_id,
                access_token,
                channel,
                release_id,
                event_type,
                event,
            )
            .await;
        lock.release().await;
        let Some(thread) = rendered? else {
            return Ok(());
        };

        for post in thread.posts {
            if !self
                .store
                .claim_slack_thread_key(integration_id, release_id, &post.key)
                .await
                .unwrap_or(false)
            {
                continue;
            }
            let payload = serde_json::json!({
                "channel": thread.channel_id,
                "thread_ts": thread.message_ts,
                "text": post.text,
                "blocks": post.blocks,
                "unfurl_links": false,
                "unfurl_media": false,
            });
            if let Err(e) = self
                .slack_call(access_token, "chat.postMessage", &payload)
                .await
            {
                let _ = self
                    .store
                    .release_slack_thread_key(integration_id, release_id, &post.key)
                    .await;
                return Err(e);
            }
        }
        Ok(())
    }

    async fn render_slack_release(
        &self,
        integration_id: &str,
        access_token: &str,
        channel: &str,
        release_id: &str,
        event_type: &str,
        event: &NotificationEvent,
    ) -> Result<Option<PendingThread>, String> {
        let is_dm = release_id.starts_with("dm:");

        let existing_ref = self
            .store
            .get_slack_message_ref(integration_id, release_id)
            .await
            .unwrap_or(None);

        let mut destinations = existing_ref
            .as_ref()
            .map(|r| r.destinations.clone())
            .unwrap_or_default();
        if let Some(r) = event.release.as_ref().filter(|r| !r.destination.is_empty()) {
            record_destination_event(&mut destinations, event_type, r, &event.timestamp);
        }

        let emoji = self
            .store
            .get_slack_emoji_set(&event.organisation)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();

        let intent_id = event
            .release
            .as_ref()
            .map(|r| r.release_intent_id.as_str())
            .unwrap_or_default();
        let (stages, steps) = match (&self.details, intent_id.is_empty()) {
            (Some(details), false) => details
                .pipeline(&event.organisation, &event.project, intent_id)
                .await?
                .unwrap_or_default(),
            _ => Default::default(),
        };
        let destination_count = existing_ref
            .as_ref()
            .map_or(0, |r| r.destination_count)
            .max(event.release.as_ref().map_or(0, |r| r.destination_count));

        let view = ReleaseView {
            event,
            destinations: &destinations,
            stages: &stages,
            steps: &steps,
            forage_url: &self.forage_url,
            emoji: &emoji,
            expected_destinations: destination_count.max(0) as usize,
        };
        let message = slack_release::render_parent(&view);
        let rendered_hash = payload_hash(&message.blocks);

        let release_title = event
            .release
            .as_ref()
            .filter(|r| !r.context_title.is_empty())
            .map(|r| r.context_title.clone())
            .or_else(|| existing_ref.as_ref().map(|r| r.release_title.clone()))
            .unwrap_or_else(|| event.title.clone());

        let mut posted: Option<(String, String)> = None;
        if let Some(ref msg_ref) = existing_ref {
            if msg_ref.rendered_hash == rendered_hash {
                posted = Some((msg_ref.channel_id.clone(), msg_ref.message_ts.clone()));
            } else {
                let payload = serde_json::json!({
                    "channel": msg_ref.channel_id,
                    "ts": msg_ref.message_ts,
                    "text": message.text,
                    "blocks": message.blocks,
                    "attachments": [],
                    "unfurl_links": false,
                    "unfurl_media": false,
                });
                match self.slack_call(access_token, "chat.update", &payload).await {
                    Ok(_) => {
                        posted = Some((msg_ref.channel_id.clone(), msg_ref.message_ts.clone()))
                    }
                    Err(e) if e.contains("message_not_found") => {
                        tracing::warn!(
                            integration_id = %integration_id,
                            release_id = %release_id,
                            "slack message not found, posting new one"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        let (channel_id, message_ts) = match posted {
            Some(p) => p,
            None => {
                let _ = self.slack_join_channel(access_token, channel).await;
                let payload = serde_json::json!({
                    "channel": channel,
                    "text": message.text,
                    "blocks": message.blocks,
                    "unfurl_links": false,
                    "unfurl_media": false,
                });
                let body = self
                    .slack_call(access_token, "chat.postMessage", &payload)
                    .await?;
                let ts = body["ts"].as_str().unwrap_or_default().to_string();
                let posted_channel = body["channel"].as_str().unwrap_or(channel).to_string();
                (posted_channel, ts)
            }
        };

        if message_ts.is_empty() || release_id.is_empty() {
            return Ok(None);
        }

        let msg_ref = SlackMessageRef {
            id: existing_ref
                .as_ref()
                .map(|r| r.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            integration_id: integration_id.to_string(),
            release_id: release_id.to_string(),
            channel_id: channel_id.clone(),
            message_ts: message_ts.clone(),
            last_event_type: event_type.to_string(),
            destinations: destinations.clone(),
            release_title,
            thread_keys: Vec::new(),
            rendered_hash,
            destination_count,
            created_at: existing_ref
                .as_ref()
                .map(|r| r.created_at.clone())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let _ = self.store.upsert_slack_message_ref(&msg_ref).await;

        let already: Vec<String> = self
            .store
            .get_slack_message_ref(integration_id, release_id)
            .await
            .ok()
            .flatten()
            .map(|r| r.thread_keys)
            .unwrap_or_default();

        let fetched = self
            .fetch_thread_detail(event, &stages, &already, is_dm)
            .await;
        let detail = ThreadDetail {
            artifacts: fetched.artifacts.as_deref(),
            plans: fetched.plans.iter().map(|(k, v)| (k.clone(), v)).collect(),
        };
        let posts = slack_release::thread_posts(&view, &detail);
        if fetched.artifacts.is_some() && !posts.iter().any(|p| p.key == "changes") {
            let _ = self
                .store
                .claim_slack_thread_key(integration_id, release_id, "changes")
                .await;
        }

        Ok(Some(PendingThread {
            channel_id,
            message_ts,
            posts: posts
                .into_iter()
                .filter(|p| !already.contains(&p.key) && (!is_dm || p.for_direct_message()))
                .collect(),
        }))
    }

    async fn fetch_thread_detail(
        &self,
        event: &NotificationEvent,
        stages: &[forage_core::platform::PipelineRunStageState],
        already: &[String],
        is_dm: bool,
    ) -> FetchedDetail {
        let mut out = FetchedDetail::default();
        let (Some(details), Some(r)) = (&self.details, event.release.as_ref()) else {
            return out;
        };
        if is_dm {
            return out;
        }
        if !already.iter().any(|k| k == "changes") {
            out.artifacts = details.artifacts(&event.organisation, &event.project).await;
        }
        for stage in stages.iter().filter(|s| s.stage_type == "plan") {
            let ready = matches!(stage.status.as_str(), "AWAITING_APPROVAL" | "SUCCEEDED");
            let key = format!("plan:{}", stage.stage_id);
            if ready
                && !already.contains(&key)
                && let Some(plan) = details
                    .plan_output(&r.release_intent_id, &stage.stage_id)
                    .await
            {
                out.plans.insert(stage.stage_id.clone(), plan);
            }
        }
        out
    }

    /// Calls a Slack Web API method, waiting out rate limits (HTTP 429 or
    /// `ratelimited`) as Slack's Retry-After asks, up to a bounded total.
    async fn slack_call(
        &self,
        access_token: &str,
        method: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut waited = 0u64;
        loop {
            let resp = self
                .http
                .post(self.slack.url(method))
                .bearer_auth(access_token)
                .json(payload)
                .send()
                .await
                .map_err(|e| format!("slack {method} http: {}", e.without_url()))?;
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let limited = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let body: serde_json::Value = if limited {
                serde_json::json!({ "ok": false, "error": "ratelimited" })
            } else {
                resp.json()
                    .await
                    .map_err(|e| format!("slack {method} parse: {e}"))?
            };
            if body["ok"].as_bool() == Some(true) {
                return Ok(body);
            }
            let err = body["error"].as_str().unwrap_or("unknown").to_string();
            if err == "ratelimited" {
                let wait = retry_after.unwrap_or(1).clamp(1, 30);
                if waited + wait <= MAX_RATE_LIMIT_WAIT_SECS {
                    tracing::warn!(method, wait_secs = wait, "slack rate limited; waiting");
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    waited += wait;
                    continue;
                }
            }
            return Err(format!("slack {method}: {err}"));
        }
    }

    async fn slack_join_channel(&self, access_token: &str, channel: &str) -> Result<(), String> {
        let payload = serde_json::json!({ "channel": channel });

        let resp = self
            .http
            .post(self.slack.url("conversations.join"))
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
            tracing::debug!(channel = %channel, error = %err, "conversations.join failed (may be private channel)");
        }

        Ok(())
    }

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

#[async_trait::async_trait]
pub trait ReleaseDetailSource: Send + Sync {
    /// `Ok(None)` when Forest has no such intent. An error fails the delivery
    /// so it is retried: rendering without the pipeline could claim success.
    async fn pipeline(
        &self,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
    ) -> Result<Option<(Vec<PipelineRunStageState>, Vec<ReleaseStepState>)>, String>;

    async fn artifacts(&self, organisation: &str, project: &str) -> Option<Vec<Artifact>>;

    async fn plan_output(&self, release_intent_id: &str, stage_id: &str) -> Option<PlanOutput>;
}

struct ForestReleaseDetails {
    grpc: Arc<GrpcForestClient>,
    service_token: String,
}

#[async_trait::async_trait]
impl ReleaseDetailSource for ForestReleaseDetails {
    async fn pipeline(
        &self,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
    ) -> Result<Option<(Vec<PipelineRunStageState>, Vec<ReleaseStepState>)>, String> {
        let intents = self
            .grpc
            .get_release_intent_states_for_notifications(
                &self.service_token,
                organisation,
                project,
            )
            .await
            .map_err(|e| format!("fetch pipeline stages: {e}"))?;
        Ok(intents
            .into_iter()
            .find(|i| i.release_intent_id == release_intent_id)
            .map(|i| (i.stages, i.steps)))
    }

    async fn artifacts(&self, organisation: &str, project: &str) -> Option<Vec<Artifact>> {
        self.grpc
            .list_artifacts(&self.service_token, organisation, project)
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "failed to list artifacts for slack thread"))
            .ok()
    }

    async fn plan_output(&self, release_intent_id: &str, stage_id: &str) -> Option<PlanOutput> {
        self.grpc
            .get_plan_output(&self.service_token, release_intent_id, stage_id)
            .await
            .inspect_err(|e| tracing::warn!(error = %e, "failed to fetch plan output for slack thread"))
            .ok()
    }
}

fn record_destination_event(
    destinations: &mut HashMap<String, DestinationStatus>,
    event_type: &str,
    r: &forage_core::integrations::router::ReleaseContext,
    at: &str,
) {
    let status = match event_type {
        "release_succeeded" => "succeeded",
        "release_failed" => "failed",
        "release_started" => "started",
        _ => return,
    };
    let previous = destinations.get(&r.destination);
    if status == "started" && previous.is_some_and(|d| d.status != "started") {
        return;
    }
    let started_at = previous
        .and_then(|d| d.started_at.clone())
        .or_else(|| Some(at.to_string()).filter(|a| !a.is_empty()));
    let completed_at = (status != "started")
        .then(|| at.to_string())
        .filter(|a| !a.is_empty());
    destinations.insert(
        r.destination.clone(),
        DestinationStatus {
            environment: r.environment.clone(),
            status: status.to_string(),
            error: r.error_message.clone(),
            started_at,
            completed_at,
        },
    );
}

fn payload_hash(blocks: &[serde_json::Value]) -> String {
    use sha2::Digest;
    let bytes = serde_json::to_vec(blocks).unwrap_or_default();
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn is_revoked_token_error(err: &str) -> bool {
    ["invalid_auth", "token_revoked", "account_inactive"]
        .iter()
        .any(|code| err.contains(code))
}

impl NotificationDispatcher {
    /// A revoked Slack token can never deliver again: stop routing to it until
    /// an admin reinstalls, rather than failing every future event.
    async fn disable_revoked(&self, task: &DispatchTask, integration_id: &str) {
        let organisation = match task {
            DispatchTask::Slack { event, .. } | DispatchTask::SlackDm { event, .. } => {
                &event.organisation
            }
            DispatchTask::Webhook { .. } => return,
        };
        let team_of = |i: &forage_core::integrations::Integration| match &i.config {
            forage_core::integrations::IntegrationConfig::Slack { team_id, .. } => {
                Some(team_id.clone())
            }
            _ => None,
        };
        let integrations = self
            .store
            .list_integrations(organisation)
            .await
            .unwrap_or_default();
        let team = integrations
            .iter()
            .find(|i| i.id == integration_id)
            .and_then(team_of)
            .filter(|t| !t.is_empty());
        for integration in &integrations {
            let same_workspace = integration.id == integration_id
                || (team.is_some() && team_of(integration) == team);
            if !same_workspace {
                continue;
            }
            if let Err(e) = self
                .store
                .set_integration_enabled(organisation, &integration.id, false)
                .await
            {
                tracing::warn!(integration_id = %integration.id, error = %e, "failed to disable revoked slack integration");
                continue;
            }
            let _ = self.store.delete_directory_matches(&integration.id).await;
            tracing::warn!(integration_id = %integration.id, "slack token revoked; integration disabled until reinstalled");
        }
    }
}

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
        source_attribution: r.source_attribution.clone(),
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

pub struct NotificationListener {
    pub grpc: Arc<GrpcForestClient>,
    pub store: Arc<dyn IntegrationStore>,
    pub service_token: String,
    pub forage_url: String,
    pub slack: crate::providers::SlackApi,
}

impl Component for NotificationListener {
    fn info(&self) -> ComponentInfo {
        "forage/notification-listener".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let dispatcher = Arc::new(
            NotificationDispatcher::new(self.store.clone(), self.forage_url.clone())
                .with_grpc(self.grpc.clone(), self.service_token.clone())
                .with_slack_api(self.slack.clone()),
        );

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

                    let recipients = crate::recipients::ForestRecipients {
                        grpc: self.grpc.clone(),
                        service_token: self.service_token.clone(),
                    };
                    let tasks = forage_core::integrations::router::route_all(
                        self.store.as_ref(),
                        Some(&recipients),
                        Some(&self.slack),
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
