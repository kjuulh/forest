use serde::{Deserialize, Serialize};

use super::router::{NotificationEvent, ReleaseContext};

/// Wire format for notification events published to NATS JetStream.
/// Mirrors `NotificationEvent` with serde support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEnvelope {
    pub id: String,
    pub notification_type: String,
    pub title: String,
    pub body: String,
    pub organisation: String,
    pub project: String,
    pub timestamp: String,
    pub release: Option<ReleaseContextEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseContextEnvelope {
    pub slug: String,
    pub artifact_id: String,
    #[serde(default)]
    pub release_intent_id: String,
    pub destination: String,
    pub environment: String,
    pub source_username: String,
    #[serde(default)]
    pub source_user_id: String,
    pub commit_sha: String,
    pub commit_branch: String,
    #[serde(default)]
    pub context_title: String,
    #[serde(default)]
    pub context_web: String,
    #[serde(default)]
    pub destination_count: i32,
    pub error_message: Option<String>,
}

impl From<&NotificationEvent> for NotificationEnvelope {
    fn from(e: &NotificationEvent) -> Self {
        Self {
            id: e.id.clone(),
            notification_type: e.notification_type.clone(),
            title: e.title.clone(),
            body: e.body.clone(),
            organisation: e.organisation.clone(),
            project: e.project.clone(),
            timestamp: e.timestamp.clone(),
            release: e.release.as_ref().map(|r| ReleaseContextEnvelope {
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
                error_message: r.error_message.clone(),
            }),
        }
    }
}

impl From<NotificationEnvelope> for NotificationEvent {
    fn from(e: NotificationEnvelope) -> Self {
        Self {
            id: e.id,
            notification_type: e.notification_type,
            title: e.title,
            body: e.body,
            organisation: e.organisation,
            project: e.project,
            timestamp: e.timestamp,
            release: e.release.map(|r| ReleaseContext {
                slug: r.slug,
                artifact_id: r.artifact_id,
                release_intent_id: r.release_intent_id,
                destination: r.destination,
                environment: r.environment,
                source_username: r.source_username,
                source_user_id: r.source_user_id,
                commit_sha: r.commit_sha,
                commit_branch: r.commit_branch,
                context_title: r.context_title,
                context_web: r.context_web,
                destination_count: r.destination_count,
                error_message: r.error_message,
            }),
        }
    }
}

/// Build the NATS subject for a notification event.
/// Format: `forage.notifications.{org}.{type}`
pub fn notification_subject(organisation: &str, notification_type: &str) -> String {
    format!("forage.notifications.{organisation}.{notification_type}")
}

/// The stream name used for notification delivery.
pub const STREAM_NAME: &str = "FORAGE_NOTIFICATIONS";

/// Subject filter for the stream (captures all orgs and types).
pub const STREAM_SUBJECTS: &str = "forage.notifications.>";

/// Durable consumer name for webhook dispatchers.
pub const CONSUMER_NAME: &str = "forage-webhook-dispatcher";

#[cfg(test)]
mod tests {
    use super::*;

    fn test_event() -> NotificationEvent {
        NotificationEvent {
            id: "notif-1".into(),
            notification_type: "release_failed".into(),
            title: "Release failed".into(),
            body: "Container timeout".into(),
            organisation: "acme-corp".into(),
            project: "my-service".into(),
            timestamp: "2026-03-09T14:30:00Z".into(),
            release: Some(ReleaseContext {
                slug: "v1.2.3".into(),
                artifact_id: "art_123".into(),
                release_intent_id: "ri_1".into(),
                destination: "prod-eu".into(),
                environment: "production".into(),
                source_username: "alice".into(),
                source_user_id: "alice_id".into(),
                commit_sha: "abc1234def".into(),
                commit_branch: "main".into(),
                context_title: "Release failed".into(),
                context_web: String::new(),
                destination_count: 3,
                error_message: Some("health check timeout".into()),
            }),
        }
    }

    #[test]
    fn envelope_roundtrip() {
        let event = test_event();
        let envelope = NotificationEnvelope::from(&event);
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: NotificationEnvelope = serde_json::from_str(&json).unwrap();
        let restored: NotificationEvent = parsed.into();

        assert_eq!(restored.id, event.id);
        assert_eq!(restored.notification_type, event.notification_type);
        assert_eq!(restored.organisation, event.organisation);
        assert_eq!(restored.project, event.project);
        let r = restored.release.unwrap();
        let orig = event.release.unwrap();
        assert_eq!(r.slug, orig.slug);
        assert_eq!(r.error_message, orig.error_message);
    }

    #[test]
    fn envelope_without_release() {
        let event = NotificationEvent {
            id: "n2".into(),
            notification_type: "release_started".into(),
            title: "Starting".into(),
            body: String::new(),
            organisation: "org".into(),
            project: "proj".into(),
            timestamp: "2026-03-09T00:00:00Z".into(),
            release: None,
        };
        let envelope = NotificationEnvelope::from(&event);
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: NotificationEnvelope = serde_json::from_str(&json).unwrap();
        let restored: NotificationEvent = parsed.into();
        assert!(restored.release.is_none());
    }

    #[test]
    fn notification_subject_format() {
        assert_eq!(
            notification_subject("acme-corp", "release_failed"),
            "forage.notifications.acme-corp.release_failed"
        );
    }
}
