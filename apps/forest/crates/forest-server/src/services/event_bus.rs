use std::collections::BTreeMap;

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

#[derive(Clone)]
pub struct EventBus {
    db: PgPool,
    nats: async_nats::Client,
}

pub struct EventPayload {
    pub organisation: String,
    pub project: String,
    pub resource_type: &'static str,
    pub action: &'static str,
    pub resource_id: String,
    pub metadata: BTreeMap<String, String>,
}

pub struct RecordedEvent {
    pub sequence: i64,
    pub event_id: Uuid,
}

impl EventBus {
    pub async fn record(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: EventPayload,
    ) -> anyhow::Result<RecordedEvent> {
        let metadata = serde_json::to_value(&event.metadata).context("serialize metadata")?;

        let rec = sqlx::query!(
            r#"INSERT INTO org_events (organisation, project, resource_type, action, resource_id, metadata)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING sequence, event_id"#,
            event.organisation,
            event.project,
            event.resource_type,
            event.action,
            event.resource_id,
            metadata,
        )
        .fetch_one(&mut **tx)
        .await
        .context("insert org_event")?;

        Ok(RecordedEvent {
            sequence: rec.sequence,
            event_id: rec.event_id,
        })
    }

    pub async fn record_standalone(&self, event: EventPayload) -> anyhow::Result<RecordedEvent> {
        let metadata = serde_json::to_value(&event.metadata).context("serialize metadata")?;

        let rec = sqlx::query!(
            r#"INSERT INTO org_events (organisation, project, resource_type, action, resource_id, metadata)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING sequence, event_id"#,
            event.organisation,
            event.project,
            event.resource_type,
            event.action,
            event.resource_id,
            metadata,
        )
        .fetch_one(&self.db)
        .await
        .context("insert org_event")?;

        Ok(RecordedEvent {
            sequence: rec.sequence,
            event_id: rec.event_id,
        })
    }

    pub async fn notify(&self, organisation: &str) {
        let subject = format!("forest.events.{}", organisation);
        if let Err(e) = self.nats.publish(subject, "".into()).await {
            tracing::warn!("failed to publish org event nudge to NATS: {e}");
        }
    }

    pub async fn emit(&self, event: EventPayload) {
        let org = event.organisation.clone();
        match self.record_standalone(event).await {
            Ok(_) => self.notify(&org).await,
            Err(e) => tracing::warn!("failed to record org event: {e:#}"),
        }
    }

    pub async fn fetch_since(
        &self,
        organisation: &str,
        since_sequence: i64,
        project: Option<&str>,
        resource_types: &[String],
        actions: &[String],
        limit: i64,
    ) -> anyhow::Result<Vec<OrgEventRow>> {
        let recs = sqlx::query_as!(
            OrgEventRow,
            r#"SELECT sequence, event_id, organisation, project, resource_type, action,
                      resource_id, metadata, created_at
               FROM org_events
               WHERE organisation = $1
                 AND sequence > $2
                 AND ($3::text IS NULL OR project = $3)
                 AND ($4::text[] IS NULL OR resource_type = ANY($4))
                 AND ($5::text[] IS NULL OR action = ANY($5))
               ORDER BY sequence ASC
               LIMIT $6"#,
            organisation,
            since_sequence,
            project,
            if resource_types.is_empty() {
                None
            } else {
                Some(resource_types)
            } as Option<&[String]>,
            if actions.is_empty() {
                None
            } else {
                Some(actions)
            } as Option<&[String]>,
            limit,
        )
        .fetch_all(&self.db)
        .await
        .context("fetch org events")?;

        Ok(recs)
    }

    pub async fn max_sequence(&self, organisation: &str) -> anyhow::Result<i64> {
        let row = sqlx::query_scalar!(
            "SELECT COALESCE(MAX(sequence), 0) FROM org_events WHERE organisation = $1",
            organisation,
        )
        .fetch_one(&self.db)
        .await
        .context("fetch max sequence")?;

        Ok(row.unwrap_or(0))
    }
}

pub struct OrgEventRow {
    pub sequence: i64,
    pub event_id: Uuid,
    pub organisation: String,
    pub project: String,
    pub resource_type: String,
    pub action: String,
    pub resource_id: String,
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub trait EventBusState {
    fn event_bus(&self) -> EventBus;
}

impl EventBusState for State {
    fn event_bus(&self) -> EventBus {
        EventBus {
            db: self.db.clone(),
            nats: self.nats.clone(),
        }
    }
}
