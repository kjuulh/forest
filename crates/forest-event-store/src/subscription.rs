use anyhow::Context;
use sqlx::{PgPool, Row};

use crate::event::RecordedEvent;
use crate::store::EventStore;

/// A catch-up subscription that tracks position in the global event log.
/// Inspired by EventStore's persistent subscriptions.
pub struct Subscription {
    pub id: String,
    store: EventStore,
    db: PgPool,
    position: i64,
    batch_size: i64,
}

impl Subscription {
    /// Create or resume a subscription. Loads last checkpoint from DB.
    pub async fn create(
        store: EventStore,
        db: PgPool,
        subscription_id: &str,
        batch_size: i64,
    ) -> anyhow::Result<Self> {
        let row = sqlx::query(
            "INSERT INTO es_subscriptions (subscription_id, last_position)
             VALUES ($1, 0)
             ON CONFLICT (subscription_id) DO UPDATE SET updated_at = now()
             RETURNING last_position",
        )
        .bind(subscription_id)
        .fetch_one(&db)
        .await
        .context("create subscription")?;

        let position: i64 = row.get("last_position");

        Ok(Self {
            id: subscription_id.to_string(),
            store,
            db,
            position,
            batch_size,
        })
    }

    /// Poll for the next batch of events from the global log.
    /// Returns an empty vec if caught up.
    pub async fn poll(&mut self) -> anyhow::Result<Vec<RecordedEvent>> {
        let events = self
            .store
            .read_all(self.position, self.batch_size)
            .await?;

        if let Some(last) = events.last() {
            self.position = last.global_position;
        }

        Ok(events)
    }

    /// Poll for events filtered by category.
    pub async fn poll_category(&mut self, category: &str) -> anyhow::Result<Vec<RecordedEvent>> {
        let events = self
            .store
            .read_category(category, self.position, self.batch_size)
            .await?;

        if let Some(last) = events.last() {
            self.position = last.global_position;
        }

        Ok(events)
    }

    /// Checkpoint current position to the database.
    /// Call this after successfully processing a batch.
    pub async fn checkpoint(&self) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE es_subscriptions SET last_position = $1, updated_at = now()
             WHERE subscription_id = $2",
        )
        .bind(self.position)
        .bind(&self.id)
        .execute(&self.db)
        .await
        .context("checkpoint subscription")?;

        tracing::debug!(
            subscription_id = %self.id,
            position = self.position,
            "checkpointed"
        );

        Ok(())
    }

    /// Current position in the global log.
    pub fn position(&self) -> i64 {
        self.position
    }
}
