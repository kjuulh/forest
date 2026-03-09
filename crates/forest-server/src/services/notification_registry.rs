use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

#[derive(Clone)]
pub struct NotificationRegistry {
    db: PgPool,
}

/// Rich context about the release that triggered the notification.
/// Stored as JSON in the database. Integrations decide which fields to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_intent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_web: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_pr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub destination_count: i32,
}

impl NotificationRegistry {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn create_notification(
        &self,
        notification_type: &str,
        title: &str,
        body: &str,
        organisation: &str,
        project: &str,
        release_context: &ReleaseContext,
    ) -> anyhow::Result<Uuid> {
        let context_json = serde_json::to_value(release_context).context("serialize context")?;

        let rec = sqlx::query!(
            r#"
            INSERT INTO notifications (
                notification_type, title, body,
                organisation, project, release_context
            ) VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
            notification_type,
            title,
            body,
            organisation,
            project,
            context_json,
        )
        .fetch_one(&self.db)
        .await
        .context("create notification")?;

        Ok(rec.id)
    }

    /// Returns the current maximum sequence number in the notifications table.
    /// Used by the listen stream to start from "now" rather than replaying history.
    pub async fn get_max_sequence(&self) -> anyhow::Result<i64> {
        let rec = sqlx::query!(
            r#"SELECT COALESCE(MAX(sequence), 0) as "max!" FROM notifications"#
        )
        .fetch_one(&self.db)
        .await
        .context("get max notification sequence")?;

        Ok(rec.max)
    }

    /// List the most recent notifications (newest first) for a user.
    /// Respects the same preference-based filtering as poll_notifications.
    pub async fn list_recent_notifications(
        &self,
        user_id: &Uuid,
        organisation: Option<&str>,
        project: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<NotificationRecord>> {
        let recs = sqlx::query!(
            r#"
            SELECT
                n.id,
                n.sequence,
                n.notification_type,
                n.title,
                n.body,
                n.organisation,
                n.project,
                n.release_context,
                n.created_at
            FROM notifications n
            WHERE ($1::text IS NULL OR n.organisation = $1)
              AND ($2::text IS NULL OR n.project = $2)
              AND NOT EXISTS (
                  SELECT 1 FROM notification_preferences np
                  WHERE np.user_id = $3
                    AND np.notification_type = n.notification_type
                    AND np.channel = 'CLI'
                    AND np.enabled = false
              )
            ORDER BY n.sequence DESC
            LIMIT $4
            "#,
            organisation,
            project,
            user_id,
            limit,
        )
        .fetch_all(&self.db)
        .await
        .context("list recent notifications")?;

        Ok(recs
            .into_iter()
            .map(|r| NotificationRecord {
                id: r.id,
                sequence: r.sequence,
                notification_type: r.notification_type,
                title: r.title,
                body: r.body,
                organisation: r.organisation,
                project: r.project,
                release_context: serde_json::from_value(r.release_context)
                    .unwrap_or_default(),
                created_at: r.created_at,
            })
            .collect())
    }

    /// Poll for notifications newer than `after_sequence` for the given user.
    /// Filters out notification types the user has explicitly disabled for CLI channel.
    pub async fn poll_notifications(
        &self,
        user_id: &Uuid,
        after_sequence: i64,
        organisation: Option<&str>,
        project: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<NotificationRecord>> {
        let recs = sqlx::query!(
            r#"
            SELECT
                n.id,
                n.sequence,
                n.notification_type,
                n.title,
                n.body,
                n.organisation,
                n.project,
                n.release_context,
                n.created_at
            FROM notifications n
            WHERE n.sequence > $1
              AND ($2::text IS NULL OR n.organisation = $2)
              AND ($3::text IS NULL OR n.project = $3)
              AND NOT EXISTS (
                  SELECT 1 FROM notification_preferences np
                  WHERE np.user_id = $4
                    AND np.notification_type = n.notification_type
                    AND np.channel = 'CLI'
                    AND np.enabled = false
              )
            ORDER BY n.sequence ASC
            LIMIT $5
            "#,
            after_sequence,
            organisation,
            project,
            user_id,
            limit,
        )
        .fetch_all(&self.db)
        .await
        .context("poll notifications")?;

        Ok(recs
            .into_iter()
            .map(|r| NotificationRecord {
                id: r.id,
                sequence: r.sequence,
                notification_type: r.notification_type,
                title: r.title,
                body: r.body,
                organisation: r.organisation,
                project: r.project,
                release_context: serde_json::from_value(r.release_context)
                    .unwrap_or_default(),
                created_at: r.created_at,
            })
            .collect())
    }

    pub async fn get_preferences(
        &self,
        user_id: &Uuid,
    ) -> anyhow::Result<Vec<NotificationPreferenceRecord>> {
        let recs = sqlx::query!(
            r#"
            SELECT id, user_id, notification_type, channel, enabled
            FROM notification_preferences
            WHERE user_id = $1
            "#,
            user_id,
        )
        .fetch_all(&self.db)
        .await
        .context("get notification preferences")?;

        Ok(recs
            .into_iter()
            .map(|r| NotificationPreferenceRecord {
                id: r.id,
                user_id: r.user_id,
                notification_type: r.notification_type,
                channel: r.channel,
                enabled: r.enabled,
            })
            .collect())
    }

    pub async fn set_preference(
        &self,
        user_id: &Uuid,
        notification_type: &str,
        channel: &str,
        enabled: bool,
    ) -> anyhow::Result<NotificationPreferenceRecord> {
        let rec = sqlx::query!(
            r#"
            INSERT INTO notification_preferences (user_id, notification_type, channel, enabled)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (user_id, notification_type, channel)
            DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now()
            RETURNING id, user_id, notification_type, channel, enabled
            "#,
            user_id,
            notification_type,
            channel,
            enabled,
        )
        .fetch_one(&self.db)
        .await
        .context("set notification preference")?;

        Ok(NotificationPreferenceRecord {
            id: rec.id,
            user_id: rec.user_id,
            notification_type: rec.notification_type,
            channel: rec.channel,
            enabled: rec.enabled,
        })
    }
}

pub trait NotificationRegistryState {
    fn notification_registry(&self) -> NotificationRegistry;
}

impl NotificationRegistryState for State {
    fn notification_registry(&self) -> NotificationRegistry {
        NotificationRegistry::new(self.db.clone())
    }
}

#[derive(Debug, Clone)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub sequence: i64,
    pub notification_type: String,
    pub title: String,
    pub body: String,
    pub organisation: String,
    pub project: String,
    pub release_context: ReleaseContext,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct NotificationPreferenceRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub notification_type: String,
    pub channel: String,
    pub enabled: bool,
}
