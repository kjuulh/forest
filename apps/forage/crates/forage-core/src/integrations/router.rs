use super::{Integration, IntegrationConfig, IntegrationStore};
use super::webhook::{ReleasePayload, WebhookPayload};

#[derive(Debug, Clone)]
pub struct NotificationEvent {
    pub id: String,
    pub notification_type: String,
    pub title: String,
    pub body: String,
    pub organisation: String,
    pub project: String,
    pub timestamp: String,
    pub release: Option<ReleaseContext>,
}

#[derive(Debug, Clone)]
pub struct ReleaseContext {
    pub slug: String,
    pub artifact_id: String,
    pub release_intent_id: String,
    pub destination: String,
    pub environment: String,
    pub source_username: String,
    pub source_user_id: String,
    /// How Forest attributed the recipient: "actor", "github_id", "email" or "" (unknown).
    pub source_attribution: String,
    pub commit_sha: String,
    pub commit_branch: String,
    pub context_title: String,
    pub context_web: String,
    pub destination_count: i32,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum DispatchTask {
    Webhook {
        integration_id: String,
        url: String,
        secret: Option<String>,
        headers: std::collections::HashMap<String, String>,
        payload: WebhookPayload,
    },
    Slack {
        integration_id: String,
        webhook_url: String,
        access_token: String,
        channel_id: String,
        release_id: String,
        notification_id: String,
        event_type: String,
        event: NotificationEvent,
        message: SlackMessage,
    },
    SlackDm {
        integration_id: String,
        access_token: String,
        slack_user_id: String,
        release_id: String,
        notification_id: String,
        event_type: String,
        event: NotificationEvent,
        message: SlackMessage,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SlackMessage {
    pub text: String,
    pub color: String,
    pub blocks: Vec<serde_json::Value>,
}

pub fn route_notification(
    event: &NotificationEvent,
    integrations: &[Integration],
) -> Vec<DispatchTask> {
    let payload = build_webhook_payload(event);

    integrations
        .iter()
        .filter_map(|integration| match &integration.config {
            IntegrationConfig::Webhook {
                url,
                secret,
                headers,
            } => Some(DispatchTask::Webhook {
                integration_id: integration.id.clone(),
                url: url.clone(),
                secret: secret.clone(),
                headers: headers.clone(),
                payload: payload.clone(),
            }),
            IntegrationConfig::Slack {
                webhook_url,
                access_token,
                channel_id,
                ..
            } => {
                let message = format_slack_message(event, &std::collections::HashMap::new(), "");
                let release_id = event
                    .release
                    .as_ref()
                    .map(|r| r.slug.clone())
                    .unwrap_or_default();
                Some(DispatchTask::Slack {
                    integration_id: integration.id.clone(),
                    webhook_url: webhook_url.clone(),
                    access_token: access_token.clone(),
                    channel_id: channel_id.clone(),
                    release_id,
                    notification_id: event.id.clone(),
                    event_type: event.notification_type.clone(),
                    event: event.clone(),
                    message,
                })
            }
            IntegrationConfig::GitHub { .. } => None,
        })
        .collect()
}

pub async fn route_notification_for_org(
    store: &dyn IntegrationStore,
    event: &NotificationEvent,
) -> Vec<DispatchTask> {
    let integrations = match store
        .list_matching_integrations(&event.organisation, &event.notification_type)
        .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(org = %event.organisation, error = %e, "failed to list matching integrations");
            return vec![];
        }
    };

    route_notification(event, &integrations)
}

/// Every delivery for an event: channel posts, webhooks and at most one DM per
/// Slack workspace. Skipped DMs are recorded in the delivery history.
pub async fn route_all(
    store: &dyn IntegrationStore,
    recipients: Option<&dyn super::direct::RecipientDirectory>,
    slack: Option<&dyn super::direct::SlackDirectory>,
    event: &NotificationEvent,
) -> Vec<DispatchTask> {
    let mut tasks = route_notification_for_org(store, event).await;
    let plan =
        super::direct::plan_direct_messages(store, recipients, slack, event, chrono::Utc::now()).await;
    super::direct::record_skips(store, event, &plan).await;
    tasks.extend(plan.tasks);
    tasks
}

fn build_webhook_payload(event: &NotificationEvent) -> WebhookPayload {
    WebhookPayload {
        event: event.notification_type.clone(),
        timestamp: event.timestamp.clone(),
        organisation: event.organisation.clone(),
        project: event.project.clone(),
        notification_id: event.id.clone(),
        title: event.title.clone(),
        body: event.body.clone(),
        release: event.release.as_ref().map(|r| ReleasePayload {
            slug: r.slug.clone(),
            artifact_id: r.artifact_id.clone(),
            destination: r.destination.clone(),
            environment: r.environment.clone(),
            source_username: r.source_username.clone(),
            commit_sha: r.commit_sha.clone(),
            commit_branch: r.commit_branch.clone(),
            error_message: r.error_message.clone(),
        }),
    }
}

pub fn format_slack_message(
    event: &NotificationEvent,
    accumulated: &std::collections::HashMap<String, super::DestinationStatus>,
    forage_url: &str,
) -> SlackMessage {
    let release = event.release.as_ref();

    let color = if accumulated.is_empty() {
        match event.notification_type.as_str() {
            "release_succeeded" => "#36a64f",
            "release_failed" => "#dc3545",
            "release_started" => "#0d6efd",
            "release_annotated" => "#6c757d",
            _ => "#6c757d",
        }
    } else {
        aggregate_color(accumulated)
    };

    let title = &event.title;

    let status_emoji = aggregate_emoji(if accumulated.is_empty() {
        &event.notification_type
    } else {
        return format_accumulated_message(event, color, accumulated, forage_url);
    });

    let text = format!("{status_emoji} {title}");

    let mut blocks: Vec<serde_json::Value> = Vec::new();

    let release_url = release
        .map(|r| build_release_url(event, r, forage_url))
        .unwrap_or_default();

    let mut header_block = serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": format!("{status_emoji} *{title}*")
        }
    });
    if !release_url.is_empty() {
        header_block["accessory"] = build_view_button(&release_url);
    }
    blocks.push(header_block);

    if let Some(r) = release.filter(|r| !r.context_title.is_empty()) {
        blocks.push(serde_json::json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!(":memo: {}", r.context_title) }
        }));
    }

    if let Some(r) = release {
        blocks.push(build_metadata_context(event, r, forage_url));
        blocks.push(serde_json::json!({ "type": "divider" }));

        if !r.destination.is_empty() {
            let dest_emoji = match event.notification_type.as_str() {
                "release_succeeded" => ":white_check_mark:",
                "release_failed" => ":x:",
                "release_started" => ":arrows_counterclockwise:",
                "release_annotated" => ":hourglass_flowing_sand:",
                _ => ":bell:",
            };
            let status_label = match event.notification_type.as_str() {
                "release_succeeded" => "Deployed",
                "release_failed" => "Failed",
                "release_started" => "Deploying",
                "release_annotated" => "Pending",
                _ => "Unknown",
            };
            let mut dest_line = format!("{dest_emoji}  `{}`  {status_label}", r.destination);
            if let Some(ref err) = r.error_message {
                dest_line.push_str(&format!(" — _{err}_"));
            }
            blocks.push(serde_json::json!({
                "type": "section",
                "text": { "type": "mrkdwn", "text": dest_line }
            }));
        }
    }

    SlackMessage {
        text,
        color: color.to_string(),
        blocks,
    }
}

fn format_accumulated_message(
    event: &NotificationEvent,
    color: &str,
    destinations: &std::collections::HashMap<String, super::DestinationStatus>,
    forage_url: &str,
) -> SlackMessage {
    let release = event.release.as_ref();
    let title = &event.title;
    let emoji = aggregate_emoji_from_destinations(destinations);
    let text = format!("{emoji} {title}");

    let mut blocks: Vec<serde_json::Value> = Vec::new();

    let release_url = release
        .map(|r| build_release_url(event, r, forage_url))
        .unwrap_or_default();

    let mut header_block = serde_json::json!({
        "type": "section",
        "text": { "type": "mrkdwn", "text": format!("{emoji} *{title}*") }
    });
    if !release_url.is_empty() {
        header_block["accessory"] = build_view_button(&release_url);
    }
    blocks.push(header_block);

    if let Some(r) = release.filter(|r| !r.context_title.is_empty()) {
        blocks.push(serde_json::json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!(":memo: {}", r.context_title) }
        }));
    }

    if let Some(r) = release {
        blocks.push(build_metadata_context(event, r, forage_url));
    }

    blocks.push(serde_json::json!({ "type": "divider" }));

    let done = destinations
        .values()
        .filter(|d| d.status == "succeeded" || d.status == "failed")
        .count();
    let total = destinations.len();

    let mut dest_parts: Vec<String> = Vec::new();
    let mut sorted_dests: Vec<_> = destinations.iter().collect();
    sorted_dests.sort_by_key(|(name, _)| name.as_str());

    for (name, status) in &sorted_dests {
        let emoji = match status.status.as_str() {
            "succeeded" => ":white_check_mark:",
            "failed" => ":x:",
            "started" => ":arrows_counterclockwise:",
            _ => ":hourglass:",
        };
        dest_parts.push(format!("{emoji} {name}"));
    }

    let dest_count = format!("  ({done}/{total})");

    blocks.push(serde_json::json!({
        "type": "section",
        "text": { "type": "mrkdwn", "text": format!("{}{dest_count}", dest_parts.join("  ")) }
    }));

    for (name, status) in &sorted_dests {
        if let Some(ref err) = status.error {
            blocks.push(serde_json::json!({
                "type": "context",
                "elements": [{ "type": "mrkdwn", "text": format!(":warning: *{name}:* {err}") }]
            }));
        }
    }

    SlackMessage {
        text,
        color: color.to_string(),
        blocks,
    }
}

fn build_metadata_context(
    event: &NotificationEvent,
    r: &ReleaseContext,
    _forage_url: &str,
) -> serde_json::Value {
    let mut parts = Vec::new();

    parts.push(format!("*{}* / {}", event.project, r.slug));

    if !r.commit_branch.is_empty() {
        parts.push(format!("`{}`", r.commit_branch));
    }
    if !r.commit_sha.is_empty() {
        parts.push(format!("`{}`", &r.commit_sha[..r.commit_sha.len().min(7)]));
    }
    if !r.source_username.is_empty() {
        parts.push(r.source_username.clone());
    }

    if !r.context_web.is_empty() {
        parts.push(format!("<{}|source>", r.context_web));
    }

    serde_json::json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": parts.join("  ·  ") }]
    })
}

fn build_release_url(
    event: &NotificationEvent,
    r: &ReleaseContext,
    forage_url: &str,
) -> String {
    if forage_url.is_empty() {
        return String::new();
    }
    format!(
        "{}/orgs/{}/projects/{}/releases/{}",
        forage_url.trim_end_matches('/'),
        event.organisation,
        event.project,
        r.slug,
    )
}

fn build_view_button(url: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "button",
        "text": { "type": "plain_text", "text": "View Release" },
        "url": url,
    })
}

pub fn format_pipeline_blocks(
    stages: &[crate::platform::PipelineRunStageState],
) -> Vec<serde_json::Value> {
    if stages.is_empty() {
        return Vec::new();
    }

    let sorted = topo_sort_stages(stages);

    let mut lines: Vec<String> = Vec::new();

    for stage in &sorted {
        let emoji = match stage.status.as_str() {
            "SUCCEEDED" => ":white_check_mark:",
            "RUNNING" => ":arrows_counterclockwise:",
            "FAILED" => ":x:",
            "CANCELLED" => ":no_entry_sign:",
            "AWAITING_APPROVAL" => ":shield:",
            _ => ":radio_button:",
        };

        let label = match stage.stage_type.as_str() {
            "deploy" => {
                let env = stage.environment.as_deref().unwrap_or("unknown");
                match stage.status.as_str() {
                    "SUCCEEDED" => format!("Deployed to `{env}`"),
                    "RUNNING" => format!("Deploying to `{env}`"),
                    "FAILED" => format!("Deploy to `{env}` failed"),
                    _ => format!("Deploy to `{env}`"),
                }
            }
            "wait" => {
                let duration = stage.duration_seconds.unwrap_or(0);
                let dur_str = if duration >= 60 {
                    format!("{}m", duration / 60)
                } else {
                    format!("{duration}s")
                };
                match stage.status.as_str() {
                    "SUCCEEDED" => format!("Waited {dur_str}"),
                    "RUNNING" => format!("Waiting {dur_str}"),
                    _ => format!("Wait {dur_str}"),
                }
            }
            "plan" => {
                let env = stage.environment.as_deref().unwrap_or("unknown");
                match stage.status.as_str() {
                    "SUCCEEDED" => format!("Plan approved for `{env}`"),
                    "RUNNING" => format!("Planning `{env}`"),
                    "AWAITING_APPROVAL" => format!("Awaiting plan approval for `{env}`"),
                    "FAILED" => format!("Plan failed for `{env}`"),
                    _ => format!("Plan `{env}`"),
                }
            }
            _ => format!("Stage {}", stage.stage_id),
        };

        let mut line = format!("{emoji}  {label}");
        if let Some(ref err) = stage.error_message {
            line.push_str(&format!(" — _{err}_"));
        }
        lines.push(line);
    }

    let done = stages
        .iter()
        .filter(|s| s.status == "SUCCEEDED" || s.status == "FAILED" || s.status == "CANCELLED")
        .count();
    let total = stages.len();

    let mut blocks = Vec::new();

    blocks.push(serde_json::json!({
        "type": "section",
        "text": { "type": "mrkdwn", "text": lines.join("\n") }
    }));

    blocks.push(serde_json::json!({
        "type": "context",
        "elements": [{ "type": "mrkdwn", "text": format!("{done}/{total} stages complete") }]
    }));

    blocks.push(serde_json::json!({ "type": "divider" }));

    blocks
}

fn topo_sort_stages(
    stages: &[crate::platform::PipelineRunStageState],
) -> Vec<crate::platform::PipelineRunStageState> {
    use std::collections::{HashMap, VecDeque};

    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for s in stages {
        in_degree.entry(s.stage_id.as_str()).or_insert(0);
        for dep in &s.depends_on {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(s.stage_id.as_str());
            *in_degree.entry(s.stage_id.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|&(_, deg)| *deg == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut sorted_ids: Vec<String> = Vec::with_capacity(stages.len());
    while let Some(id) = queue.pop_front() {
        sorted_ids.push(id.to_string());
        if let Some(deps) = dependents.get(id) {
            for &dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    let id_to_idx: HashMap<&str, usize> = sorted_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    let mut indexed: Vec<(usize, &crate::platform::PipelineRunStageState)> = stages
        .iter()
        .map(|s| {
            let idx = id_to_idx
                .get(s.stage_id.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            (idx, s)
        })
        .collect();
    indexed.sort_by_key(|(idx, _)| *idx);

    indexed.into_iter().map(|(_, s)| s.clone()).collect()
}

fn aggregate_color(
    destinations: &std::collections::HashMap<String, super::DestinationStatus>,
) -> &'static str {
    let has_failed = destinations.values().any(|d| d.status == "failed");
    let has_started = destinations.values().any(|d| d.status == "started");
    if has_failed {
        "#dc3545"
    } else if has_started {
        "#0d6efd"
    } else {
        "#36a64f"
    }
}

fn aggregate_emoji(event_type: &str) -> &'static str {
    match event_type {
        "release_succeeded" => ":white_check_mark:",
        "release_failed" => ":x:",
        "release_started" => ":rocket:",
        "release_annotated" => ":hourglass_flowing_sand:",
        _ => ":bell:",
    }
}

fn aggregate_emoji_from_destinations(
    destinations: &std::collections::HashMap<String, super::DestinationStatus>,
) -> &'static str {
    let has_failed = destinations.values().any(|d| d.status == "failed");
    let has_started = destinations.values().any(|d| d.status == "started");
    if has_failed {
        ":x:"
    } else if has_started {
        ":rocket:"
    } else {
        ":white_check_mark:"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_event() -> NotificationEvent {
        NotificationEvent {
            id: "notif-1".into(),
            notification_type: "release_failed".into(),
            title: "Release failed".into(),
            body: "Container timeout".into(),
            organisation: "test-org".into(),
            project: "my-project".into(),
            timestamp: "2026-03-09T14:30:00Z".into(),
            release: Some(ReleaseContext {
                slug: "test-release".into(),
                artifact_id: "art_123".into(),
                release_intent_id: "ri_1".into(),
                destination: "prod-eu".into(),
                environment: "production".into(),
                source_username: "alice".into(),
                source_user_id: "alice_id".into(),
                source_attribution: "actor".into(),
                commit_sha: "abc1234def".into(),
                commit_branch: "main".into(),
                context_title: "Release failed".into(),
                context_web: String::new(),
                destination_count: 3,
                error_message: Some("health check timeout".into()),
            }),
        }
    }

    fn webhook_integration(id: &str) -> Integration {
        Integration {
            id: id.into(),
            organisation: "test-org".into(),
            integration_type: super::super::IntegrationType::Webhook,
            name: "prod-alerts".into(),
            config: IntegrationConfig::Webhook {
                url: "https://hooks.example.com/test".into(),
                secret: Some("s3cret".into()),
                headers: HashMap::new(),
            },
            enabled: true,
            created_by: "user-1".into(),
            created_at: "2026-03-09T00:00:00Z".into(),
            updated_at: "2026-03-09T00:00:00Z".into(),
            api_token: None,
        }
    }

    fn slack_integration(id: &str) -> Integration {
        Integration {
            id: id.into(),
            organisation: "test-org".into(),
            integration_type: super::super::IntegrationType::Slack,
            name: "#deploys".into(),
            config: IntegrationConfig::Slack {
                team_id: "T123".into(),
                team_name: "Test".into(),
                channel_id: "C456".into(),
                channel_name: "#deploys".into(),
                access_token: "xoxb-test".into(),
                webhook_url: "https://hooks.slack.com/test".into(),
            },
            enabled: true,
            created_by: "user-1".into(),
            created_at: "2026-03-09T00:00:00Z".into(),
            updated_at: "2026-03-09T00:00:00Z".into(),
            api_token: None,
        }
    }

    #[test]
    fn route_to_webhook() {
        let event = test_event();
        let integrations = vec![webhook_integration("w1")];
        let tasks = route_notification(&event, &integrations);

        assert_eq!(tasks.len(), 1);
        match &tasks[0] {
            DispatchTask::Webhook {
                integration_id,
                url,
                secret,
                payload,
                ..
            } => {
                assert_eq!(integration_id, "w1");
                assert_eq!(url, "https://hooks.example.com/test");
                assert_eq!(secret.as_deref(), Some("s3cret"));
                assert_eq!(payload.event, "release_failed");
                assert_eq!(payload.organisation, "test-org");
            }
            _ => panic!("expected Webhook task"),
        }
    }

    #[test]
    fn route_to_slack() {
        let event = test_event();
        let integrations = vec![slack_integration("s1")];
        let tasks = route_notification(&event, &integrations);

        assert_eq!(tasks.len(), 1);
        match &tasks[0] {
            DispatchTask::Slack {
                integration_id,
                message,
                ..
            } => {
                assert_eq!(integration_id, "s1");
                assert!(message.text.contains("Release failed"));
                assert_eq!(message.color, "#dc3545");
            }
            _ => panic!("expected Slack task"),
        }
    }

    #[test]
    fn route_to_multiple_integrations() {
        let event = test_event();
        let integrations = vec![webhook_integration("w1"), slack_integration("s1")];
        let tasks = route_notification(&event, &integrations);
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn route_to_empty_integrations() {
        let event = test_event();
        let tasks = route_notification(&event, &[]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn slack_message_annotated_reads_as_pending() {
        let mut event = test_event();
        event.notification_type = "release_annotated".into();
        event.title = "Release pending: understory/commission-dashboard".into();

        let msg = format_slack_message(&event, &std::collections::HashMap::new(), "");

        assert!(
            msg.text.starts_with(":hourglass_flowing_sand:"),
            "expected an hourglass in the fallback text, got {:?}",
            msg.text
        );
        assert!(
            msg.text.contains("Release pending"),
            "expected the pending title, got {:?}",
            msg.text
        );
        assert_eq!(msg.color, "#6c757d", "pending stays grey, not green or red");
    }

    #[test]
    fn slack_message_color_success() {
        let mut event = test_event();
        event.notification_type = "release_succeeded".into();
        let msg = format_slack_message(&event, &HashMap::new(), "");
        assert_eq!(msg.color, "#36a64f");
    }

    #[test]
    fn slack_message_includes_error() {
        let event = test_event();
        let msg = format_slack_message(&event, &HashMap::new(), "");
        let blocks_str = serde_json::to_string(&msg.blocks).unwrap();
        assert!(blocks_str.contains("health check timeout"));
    }

    #[test]
    fn slack_message_accumulated_shows_all_destinations() {
        let event = test_event();
        let mut dests = HashMap::new();
        dests.insert("prod-eu".into(), super::super::DestinationStatus {
            environment: "production".into(),
            status: "succeeded".into(),
            error: None,
        });
        dests.insert("staging".into(), super::super::DestinationStatus {
            environment: "staging".into(),
            status: "started".into(),
            error: None,
        });
        let msg = format_slack_message(&event, &dests, "");
        assert_eq!(msg.color, "#0d6efd");
        let blocks_str = serde_json::to_string(&msg.blocks).unwrap();
        assert!(blocks_str.contains("prod-eu"));
        assert!(blocks_str.contains("staging"));
    }

    #[test]
    fn slack_message_accumulated_all_succeeded() {
        let mut event = test_event();
        if let Some(ref mut r) = event.release {
            r.destination_count = 2;
        }
        let mut dests = HashMap::new();
        dests.insert("prod-eu".into(), super::super::DestinationStatus {
            environment: "production".into(),
            status: "succeeded".into(),
            error: None,
        });
        dests.insert("staging".into(), super::super::DestinationStatus {
            environment: "staging".into(),
            status: "succeeded".into(),
            error: None,
        });
        let msg = format_slack_message(&event, &dests, "");
        assert_eq!(msg.color, "#36a64f");
    }

    #[test]
    fn slack_message_in_progress_is_blue() {
        let event = test_event();
        let mut dests = HashMap::new();
        dests.insert("prod-eu".into(), super::super::DestinationStatus {
            environment: "production".into(),
            status: "started".into(),
            error: None,
        });
        let msg = format_slack_message(&event, &dests, "");
        assert_eq!(msg.color, "#0d6efd");
    }

    #[test]
    fn slack_message_accumulated_shows_errors() {
        let event = test_event();
        let mut dests = HashMap::new();
        dests.insert("prod-eu".into(), super::super::DestinationStatus {
            environment: "production".into(),
            status: "failed".into(),
            error: Some("OOM killed".into()),
        });
        let msg = format_slack_message(&event, &dests, "");
        assert_eq!(msg.color, "#dc3545");
        let blocks_str = serde_json::to_string(&msg.blocks).unwrap();
        assert!(blocks_str.contains("OOM killed"));
    }

    #[test]
    fn pipeline_blocks_renders_stages() {
        use crate::platform::PipelineRunStageState;

        let stages = vec![
            PipelineRunStageState {
                stage_id: "s1".into(),
                depends_on: vec![],
                stage_type: "deploy".into(),
                status: "SUCCEEDED".into(),
                environment: Some("dev".into()),
                ..Default::default()
            },
            PipelineRunStageState {
                stage_id: "s2".into(),
                depends_on: vec!["s1".into()],
                stage_type: "wait".into(),
                status: "SUCCEEDED".into(),
                duration_seconds: Some(3),
                ..Default::default()
            },
            PipelineRunStageState {
                stage_id: "s3".into(),
                depends_on: vec!["s2".into()],
                stage_type: "deploy".into(),
                status: "RUNNING".into(),
                environment: Some("staging".into()),
                ..Default::default()
            },
            PipelineRunStageState {
                stage_id: "s4".into(),
                depends_on: vec!["s3".into()],
                stage_type: "wait".into(),
                status: "PENDING".into(),
                duration_seconds: Some(5),
                ..Default::default()
            },
            PipelineRunStageState {
                stage_id: "s5".into(),
                depends_on: vec!["s4".into()],
                stage_type: "deploy".into(),
                status: "PENDING".into(),
                environment: Some("prod".into()),
                ..Default::default()
            },
        ];

        let blocks = format_pipeline_blocks(&stages);
        assert_eq!(blocks.len(), 3);

        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("Deployed to `dev`"));
        assert!(text.contains("Waited 3s"));
        assert!(text.contains("Deploying to `staging`"));
        assert!(text.contains("Wait 5s"));
        assert!(text.contains("Deploy to `prod`"));

        let progress = blocks[1]["elements"][0]["text"].as_str().unwrap();
        assert_eq!(progress, "2/5 stages complete");
    }

    #[test]
    fn pipeline_blocks_empty_stages_returns_nothing() {
        let blocks = format_pipeline_blocks(&[]);
        assert!(blocks.is_empty());
    }

    #[test]
    fn pipeline_blocks_shows_errors() {
        use crate::platform::PipelineRunStageState;

        let stages = vec![PipelineRunStageState {
            stage_id: "s1".into(),
            depends_on: vec![],
            stage_type: "deploy".into(),
            status: "FAILED".into(),
            environment: Some("prod".into()),
            error_message: Some("OOM killed".into()),
            ..Default::default()
        }];

        let blocks = format_pipeline_blocks(&stages);
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("Deploy to `prod` failed"));
        assert!(text.contains("OOM killed"));
    }
}
