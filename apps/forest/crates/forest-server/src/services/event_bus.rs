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
    /// Record an event inside an existing transaction. Call this before commit.
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

    /// Record an event using the pool directly (non-transactional).
    /// Use when the caller doesn't have an open transaction.
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

    /// Send a best-effort NATS nudge to wake any listeners for this org.
    /// Call after transaction commit.
    pub async fn notify(&self, organisation: &str) {
        let subject = format!("forest.events.{}", organisation);
        if let Err(e) = self.nats.publish(subject, "".into()).await {
            tracing::warn!("failed to publish org event nudge to NATS: {e}");
        }
    }

    /// Convenience: record (standalone) + notify in one call.
    /// For use in services that don't have an open transaction.
    pub async fn emit(&self, event: EventPayload) {
        let org = event.organisation.clone();
        match self.record_standalone(event).await {
            Ok(_) => self.notify(&org).await,
            Err(e) => tracing::warn!("failed to record org event: {e:#}"),
        }
    }

    /// Fetch events for an organisation after a given sequence.
    pub async fn fetch_since(
        &self,
        organisation: &str,
        since_sequence: i64,
        project: Option<&str>,
        resource_types: &[String],
        actions: &[String],
        limit: i64,
    ) -> anyhow::Result<Vec<OrgEventRow>> {
        // We build a simple query with optional filters.
        // For simplicity and sqlx compatibility, we use a single query with array contains.
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

    /// Get the current max sequence for an organisation (for "latest only" subscriptions).
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
