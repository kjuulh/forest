use anyhow::{Context, bail};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::{
    Aggregate, AggregateRoot,
    event::{EventData, RecordedEvent},
    stream::{ExpectedVersion, ReadDirection, StreamQuery},
};

/// PostgreSQL-backed event store.
#[derive(Clone)]
pub struct EventStore {
    db: PgPool,
}

impl EventStore {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub fn pool(&self) -> &PgPool {
        &self.db
    }

    /// Ensure event store tables exist. Idempotent and concurrency-safe.
    /// Uses a PG advisory lock to serialize concurrent migration attempts.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;

        // Advisory lock (session-scoped within tx) prevents concurrent migration races
        sqlx::query("SELECT pg_advisory_xact_lock(7_413_952_871)")
            .execute(&mut *tx)
            .await
            .context("advisory lock for migration")?;

        let sql = include_str!("../migrations/20260309000001_event_store.sql");
        for statement in sql.split(';') {
            let cleaned: String = statement
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            let cleaned = cleaned.trim();
            if cleaned.is_empty() {
                continue;
            }
            sqlx::query(cleaned)
                .execute(&mut *tx)
                .await
                .with_context(|| {
                    format!(
                        "event store migration: {}",
                        &cleaned[..cleaned.len().min(80)]
                    )
                })?;
        }

        tx.commit().await.context("commit migration")?;
        Ok(())
    }

    /// Load an aggregate by replaying all events from its stream.
    /// Returns `None` if the stream doesn't exist.
    pub async fn load<A: Aggregate>(&self, id: &str) -> anyhow::Result<Option<AggregateRoot<A>>> {
        let category = A::stream_category();
        let stream_id = format!("{}-{}", category, id);

        let row = sqlx::query("SELECT stream_version FROM es_streams WHERE stream_id = $1")
            .bind(&stream_id)
            .fetch_optional(&self.db)
            .await
            .context("load stream")?;

        let Some(row) = row else {
            return Ok(None);
        };

        let version: i64 = row.get("stream_version");

        let events = self
            .read_stream(&stream_id, &StreamQuery::default())
            .await?;

        Ok(Some(AggregateRoot::hydrate(stream_id, &events, version)))
    }

    /// Load an aggregate, creating a new one if the stream doesn't exist.
    pub async fn load_or_default<A: Aggregate>(
        &self,
        id: &str,
    ) -> anyhow::Result<AggregateRoot<A>> {
        let category = A::stream_category();
        let stream_id = format!("{}-{}", category, id);

        match self.load::<A>(id).await? {
            Some(root) => Ok(root),
            None => Ok(AggregateRoot::new(stream_id)),
        }
    }

    /// Persist pending events from an aggregate root.
    /// Uses optimistic concurrency: fails if the stream was modified since loading.
    pub async fn save<A: Aggregate>(&self, root: &mut AggregateRoot<A>) -> anyhow::Result<()> {
        let events = root.take_pending();
        if events.is_empty() {
            return Ok(());
        }

        let expected = if root.version == 0 {
            ExpectedVersion::NoStream
        } else {
            ExpectedVersion::Exact(root.version)
        };

        let category = A::stream_category();
        let new_version = self
            .append(&root.stream_id, category.as_str(), expected, &events)
            .await?;

        root.version = new_version;
        Ok(())
    }

    /// Save with a transactional side-effect. The closure receives the open
    /// transaction *after* events have been appended but *before* commit.
    /// Use this to update projections/read-models atomically with the events.
    ///
    /// If the closure returns an error, the entire transaction (events + projection) is rolled back.
    pub async fn save_with<A, F>(
        &self,
        root: &mut AggregateRoot<A>,
        f: F,
    ) -> anyhow::Result<()>
    where
        A: Aggregate,
        for<'t> F: FnOnce(
            &[A::Event],
            &'t mut Transaction<'_, Postgres>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 't>,
        >,
    {
        let events = root.take_pending();
        if events.is_empty() {
            return Ok(());
        }

        let expected = if root.version == 0 {
            ExpectedVersion::NoStream
        } else {
            ExpectedVersion::Exact(root.version)
        };

        let mut tx = self.db.begin().await?;

        let category = A::stream_category();
        let new_version = self
            .append_in_tx(&mut tx, &root.stream_id, category.as_str(), expected, &events)
            .await?;

        // Run the projection/side-effect inside the same transaction
        f(&events, &mut tx).await?;

        tx.commit().await.context("commit save_with")?;

        root.version = new_version;

        Ok(())
    }

    /// Low-level append: write events to a stream with concurrency control.
    /// Returns the new stream version after append.
    pub async fn append<E: EventData>(
        &self,
        stream_id: &str,
        category: &str,
        expected: ExpectedVersion,
        events: &[E],
    ) -> anyhow::Result<i64> {
        if events.is_empty() {
            bail!("cannot append zero events");
        }

        let mut tx = self.db.begin().await?;

        let version = self
            .append_in_tx(&mut tx, stream_id, category, expected, events)
            .await?;

        tx.commit().await.context("commit append")?;

        Ok(version)
    }

    /// Append events within an existing transaction. Used by `save_with` for
    /// atomic projection updates and by `append` for standalone writes.
    pub async fn append_in_tx<E: EventData>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        stream_id: &str,
        category: &str,
        expected: ExpectedVersion,
        events: &[E],
    ) -> anyhow::Result<i64> {
        if events.is_empty() {
            bail!("cannot append zero events");
        }

        // Upsert stream and lock row
        let row = sqlx::query(
            "INSERT INTO es_streams (stream_id, stream_category, stream_version)
             VALUES ($1, $2, 0)
             ON CONFLICT (stream_id) DO UPDATE SET updated_at = now()
             RETURNING stream_version",
        )
        .bind(stream_id)
        .bind(category)
        .fetch_one(&mut **tx)
        .await
        .context("upsert stream")?;

        let current_version: i64 = row.get("stream_version");

        // Check expected version
        match expected {
            ExpectedVersion::NoStream => {
                if current_version != 0 {
                    bail!(
                        "expected no stream but stream '{}' exists at version {}",
                        stream_id,
                        current_version
                    );
                }
            }
            ExpectedVersion::Exact(v) => {
                if current_version != v {
                    bail!(
                        "concurrency conflict on stream '{}': expected version {} but found {}",
                        stream_id,
                        v,
                        current_version
                    );
                }
            }
            ExpectedVersion::Any => {}
        }

        let mut version = current_version;
        for event in events {
            version += 1;
            let event_type = event.event_type();
            let data = serde_json::to_value(event).context("serialize event data")?;

            sqlx::query(
                "INSERT INTO es_events (stream_id, stream_version, event_type, data)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(stream_id)
            .bind(version)
            .bind(event_type)
            .bind(&data)
            .execute(&mut **tx)
            .await
            .context("insert event")?;
        }

        // Update stream version
        sqlx::query(
            "UPDATE es_streams SET stream_version = $1, updated_at = now()
             WHERE stream_id = $2",
        )
        .bind(version)
        .bind(stream_id)
        .execute(&mut **tx)
        .await
        .context("update stream version")?;

        tracing::debug!(
            stream_id,
            old_version = current_version,
            new_version = version,
            event_count = events.len(),
            "appended events"
        );

        Ok(version)
    }

    /// Read events from a specific stream.
    pub async fn read_stream(
        &self,
        stream_id: &str,
        query: &StreamQuery,
    ) -> anyhow::Result<Vec<RecordedEvent>> {
        let rows = match query.direction {
            ReadDirection::Forward => {
                sqlx::query(
                    "SELECT global_position, stream_id, stream_version, event_type,
                            data, metadata, created_at
                     FROM es_events
                     WHERE stream_id = $1 AND stream_version >= $2
                     ORDER BY stream_version ASC
                     LIMIT $3",
                )
                .bind(stream_id)
                .bind(query.from_version)
                .bind(query.limit)
                .fetch_all(&self.db)
                .await
                .context("read stream forward")?
            }
            ReadDirection::Backward => {
                sqlx::query(
                    "SELECT global_position, stream_id, stream_version, event_type,
                            data, metadata, created_at
                     FROM es_events
                     WHERE stream_id = $1 AND stream_version <= $2
                     ORDER BY stream_version DESC
                     LIMIT $3",
                )
                .bind(stream_id)
                .bind(query.from_version)
                .bind(query.limit)
                .fetch_all(&self.db)
                .await
                .context("read stream backward")?
            }
        };

        Ok(rows.into_iter().map(row_to_recorded_event).collect())
    }

    /// Read events across all streams by global position (for projections/subscriptions).
    pub async fn read_all(
        &self,
        from_position: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<RecordedEvent>> {
        let rows = sqlx::query(
            "SELECT global_position, stream_id, stream_version, event_type,
                    data, metadata, created_at
             FROM es_events
             WHERE global_position > $1
             ORDER BY global_position ASC
             LIMIT $2",
        )
        .bind(from_position)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .context("read all events")?;

        Ok(rows.into_iter().map(row_to_recorded_event).collect())
    }

    /// Read events filtered by category (all streams sharing a prefix).
    pub async fn read_category(
        &self,
        category: &str,
        from_position: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<RecordedEvent>> {
        let rows = sqlx::query(
            "SELECT e.global_position, e.stream_id, e.stream_version, e.event_type,
                    e.data, e.metadata, e.created_at
             FROM es_events e
             JOIN es_streams s ON s.stream_id = e.stream_id
             WHERE s.stream_category = $1 AND e.global_position > $2
             ORDER BY e.global_position ASC
             LIMIT $3",
        )
        .bind(category)
        .bind(from_position)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .context("read category events")?;

        Ok(rows.into_iter().map(row_to_recorded_event).collect())
    }
}

fn row_to_recorded_event(row: sqlx::postgres::PgRow) -> RecordedEvent {
    RecordedEvent {
        global_position: row.get("global_position"),
        stream_id: row.get("stream_id"),
        stream_version: row.get("stream_version"),
        event_type: row.get("event_type"),
        data: row.get("data"),
        metadata: row.get("metadata"),
        created_at: row.get("created_at"),
    }
}
