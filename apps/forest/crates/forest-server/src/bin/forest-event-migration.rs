use std::collections::BTreeMap;

use anyhow::Context;
use clap::{Parser, Subcommand};
use forest_server::domains::{
    app::AppEvent, component::ComponentEvent, destination::DestinationEvent,
    device_login::DeviceGrantEvent, policy::PolicyEvent, trigger::TriggerEvent,
};
use futures::TryStreamExt;
use mire::EventData;
use serde::Serialize;
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool, Row, postgres::PgPoolOptions};

const SUPPORTED_CATEGORIES: [&str; 6] = [
    "app",
    "component",
    "destination",
    "device_grant",
    "policy",
    "trigger",
];

#[derive(Debug, Parser)]
#[command(
    name = "forest-event-migration",
    about = "Audit Forest event data before and after the Mire schema migration"
)]
struct Args {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply Forest migrations and finish compatibility backfills after legacy writers stop.
    Prepare,

    /// Inspect event schema, stream integrity, payload compatibility, and projection coverage.
    Audit {
        /// Exit unsuccessfully when the database is not ready for Mire.
        #[arg(long)]
        strict: bool,

        /// Maximum number of decode failures included in the JSON report.
        #[arg(long, default_value_t = 20)]
        max_error_samples: usize,
    },
}

#[derive(Debug, Serialize)]
struct AuditReport {
    server_version: String,
    server_version_num: i32,
    mire_supported_postgres: bool,
    counts: Counts,
    stream_categories: BTreeMap<String, i64>,
    compatibility_columns: CompatibilityColumns,
    compatibility_nulls: CompatibilityNulls,
    invariants: Invariants,
    projection_rows_without_stream: BTreeMap<String, i64>,
    sequence: SequenceAudit,
    decode: DecodeAudit,
    mire_compatible: bool,
}

#[derive(Debug, Serialize)]
struct Counts {
    streams: i64,
    events: i64,
    subscriptions: i64,
}

#[derive(Debug, Serialize)]
struct CompatibilityColumns {
    event_transaction_id: bool,
    event_stream_category: bool,
    event_payload_size: bool,
    event_metadata_size: bool,
    subscription_transaction_id: bool,
}

impl CompatibilityColumns {
    fn complete(&self) -> bool {
        self.event_transaction_id
            && self.event_stream_category
            && self.event_payload_size
            && self.event_metadata_size
            && self.subscription_transaction_id
    }
}

#[derive(Debug, Default, Serialize)]
struct CompatibilityNulls {
    event_transaction_id: Option<i64>,
    event_stream_category: Option<i64>,
    event_payload_size: Option<i64>,
    event_metadata_size: Option<i64>,
    subscription_transaction_id: Option<i64>,
}

impl CompatibilityNulls {
    fn empty(&self) -> bool {
        [
            self.event_transaction_id,
            self.event_stream_category,
            self.event_payload_size,
            self.event_metadata_size,
            self.subscription_transaction_id,
        ]
        .into_iter()
        .all(|count| count == Some(0))
    }
}

#[derive(Debug, Serialize)]
struct Invariants {
    events_without_stream: i64,
    stream_version_mismatches: i64,
    non_contiguous_streams: i64,
    event_category_mismatches: Option<i64>,
    subscription_cursor_mismatches: Option<i64>,
}

impl Invariants {
    fn valid(&self) -> bool {
        self.events_without_stream == 0
            && self.stream_version_mismatches == 0
            && self.non_contiguous_streams == 0
            && self.event_category_mismatches == Some(0)
            && self.subscription_cursor_mismatches == Some(0)
    }
}

#[derive(Debug, Serialize)]
struct SequenceAudit {
    last_value: i64,
    is_called: bool,
    maximum_global_position: i64,
    safe_for_next_insert: bool,
}

#[derive(Debug, Serialize)]
struct DecodeAudit {
    checked: i64,
    failures: i64,
    samples: Vec<DecodeFailure>,
}

#[derive(Debug, Serialize)]
struct DecodeFailure {
    global_position: i64,
    stream_id: String,
    stream_category: String,
    stored_event_type: String,
    error: String,
}

#[derive(Debug, Serialize)]
struct PrepareReport {
    event_transaction_ids_backfilled: u64,
    event_categories_backfilled: u64,
    event_sizes_backfilled: u64,
    subscription_cursors_rebuilt: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&args.database_url)
        .await
        .context("connect to the Forest database")?;

    match args.command {
        Command::Prepare => {
            let report = prepare(&pool).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Audit {
            strict,
            max_error_samples,
        } => {
            let report = audit(&pool, max_error_samples).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);

            if strict && !report.mire_compatible {
                anyhow::bail!("event store is not ready for Mire; inspect the JSON audit report");
            }
        }
    }

    Ok(())
}

async fn prepare(pool: &PgPool) -> anyhow::Result<PrepareReport> {
    sqlx::migrate!("./migrations/")
        .run(pool)
        .await
        .context("apply Forest schema migrations")?;

    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(7_413_952_871_i64)
        .execute(&mut *tx)
        .await
        .context("lock event migration backfill")?;

    let event_transaction_ids_backfilled = sqlx::query(
        "UPDATE es_events
            SET transaction_id = pg_current_xact_id()
          WHERE transaction_id IS NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let event_categories_backfilled = sqlx::query(
        "UPDATE es_events event
            SET stream_category = stream.stream_category
           FROM es_streams stream
          WHERE event.stream_id = stream.stream_id
            AND event.stream_category IS NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let event_sizes_backfilled = sqlx::query(
        "UPDATE es_events
            SET payload_size = COALESCE(payload_size, octet_length(data::text)),
                metadata_size = COALESCE(metadata_size, octet_length(metadata::text))
          WHERE payload_size IS NULL
             OR metadata_size IS NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let subscription_cursors_rebuilt = sqlx::query(
        "UPDATE es_subscriptions subscription
            SET last_transaction_id = COALESCE(
                (
                    SELECT event.transaction_id
                      FROM es_events event
                     WHERE event.global_position <= subscription.last_position
                     ORDER BY event.global_position DESC
                     LIMIT 1
                ),
                '0'::xid8
            )",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;

    Ok(PrepareReport {
        event_transaction_ids_backfilled,
        event_categories_backfilled,
        event_sizes_backfilled,
        subscription_cursors_rebuilt,
    })
}

async fn audit(pool: &PgPool, max_error_samples: usize) -> anyhow::Result<AuditReport> {
    let server_version: String = sqlx::query_scalar("SELECT version()")
        .fetch_one(pool)
        .await?;
    let server_version_num: i32 =
        sqlx::query_scalar("SELECT current_setting('server_version_num')::integer")
            .fetch_one(pool)
            .await?;

    let counts = Counts {
        streams: scalar_count(pool, "SELECT count(*)::bigint FROM es_streams").await?,
        events: scalar_count(pool, "SELECT count(*)::bigint FROM es_events").await?,
        subscriptions: scalar_count(pool, "SELECT count(*)::bigint FROM es_subscriptions").await?,
    };

    let compatibility_columns = CompatibilityColumns {
        event_transaction_id: column_exists(pool, "es_events", "transaction_id").await?,
        event_stream_category: column_exists(pool, "es_events", "stream_category").await?,
        event_payload_size: column_exists(pool, "es_events", "payload_size").await?,
        event_metadata_size: column_exists(pool, "es_events", "metadata_size").await?,
        subscription_transaction_id: column_exists(pool, "es_subscriptions", "last_transaction_id")
            .await?,
    };

    let compatibility_nulls = CompatibilityNulls {
        event_transaction_id: nullable_count(
            pool,
            "es_events",
            "transaction_id",
            compatibility_columns.event_transaction_id,
        )
        .await?,
        event_stream_category: nullable_count(
            pool,
            "es_events",
            "stream_category",
            compatibility_columns.event_stream_category,
        )
        .await?,
        event_payload_size: nullable_count(
            pool,
            "es_events",
            "payload_size",
            compatibility_columns.event_payload_size,
        )
        .await?,
        event_metadata_size: nullable_count(
            pool,
            "es_events",
            "metadata_size",
            compatibility_columns.event_metadata_size,
        )
        .await?,
        subscription_transaction_id: nullable_count(
            pool,
            "es_subscriptions",
            "last_transaction_id",
            compatibility_columns.subscription_transaction_id,
        )
        .await?,
    };

    let events_without_stream = scalar_count(
        pool,
        "SELECT count(*)::bigint
           FROM es_events event
           LEFT JOIN es_streams stream USING (stream_id)
          WHERE stream.stream_id IS NULL",
    )
    .await?;

    let stream_version_mismatches = scalar_count(
        pool,
        "WITH actual AS (
             SELECT stream_id, max(stream_version) AS maximum_version
               FROM es_events
              GROUP BY stream_id
         )
         SELECT count(*)::bigint
           FROM es_streams stream
           LEFT JOIN actual USING (stream_id)
          WHERE stream.stream_version <> COALESCE(actual.maximum_version, 0)",
    )
    .await?;

    let non_contiguous_streams = scalar_count(
        pool,
        "SELECT count(*)::bigint
           FROM (
             SELECT stream_id,
                    min(stream_version) AS minimum_version,
                    max(stream_version) AS maximum_version,
                    count(*)::bigint AS event_count
               FROM es_events
              GROUP BY stream_id
             HAVING min(stream_version) <> 1
                 OR count(*)::bigint <> max(stream_version)
           ) broken",
    )
    .await?;

    let event_category_mismatches = if compatibility_columns.event_stream_category {
        Some(
            scalar_count(
                pool,
                "SELECT count(*)::bigint
                   FROM es_events event
                   JOIN es_streams stream USING (stream_id)
                  WHERE event.stream_category IS DISTINCT FROM stream.stream_category",
            )
            .await?,
        )
    } else {
        None
    };

    let subscription_cursor_mismatches = if compatibility_columns.subscription_transaction_id {
        Some(
            scalar_count(
                pool,
                "SELECT count(*)::bigint
                       FROM es_subscriptions subscription
                      WHERE subscription.last_transaction_id IS DISTINCT FROM COALESCE(
                            (
                                SELECT event.transaction_id
                                  FROM es_events event
                                 WHERE event.global_position <= subscription.last_position
                                 ORDER BY event.global_position DESC
                                 LIMIT 1
                            ),
                            '0'::xid8
                      )",
            )
            .await?,
        )
    } else {
        None
    };

    let invariants = Invariants {
        events_without_stream,
        stream_version_mismatches,
        non_contiguous_streams,
        event_category_mismatches,
        subscription_cursor_mismatches,
    };

    let stream_categories = stream_categories(pool).await?;
    let projection_rows_without_stream = projection_coverage(pool).await?;
    let projection_coverage_complete = projection_rows_without_stream
        .values()
        .all(|count| *count == 0);
    let sequence = sequence_audit(pool).await?;
    let decode = decode_events(pool, max_error_samples).await?;

    let mire_supported_postgres = server_version_num >= 130_000;
    let mire_compatible = mire_supported_postgres
        && compatibility_columns.complete()
        && compatibility_nulls.empty()
        && invariants.valid()
        && projection_coverage_complete
        && sequence.safe_for_next_insert
        && decode.failures == 0;

    Ok(AuditReport {
        server_version,
        server_version_num,
        mire_supported_postgres,
        counts,
        stream_categories,
        compatibility_columns,
        compatibility_nulls,
        invariants,
        projection_rows_without_stream,
        sequence,
        decode,
        mire_compatible,
    })
}

async fn column_exists(pool: &PgPool, table: &str, column: &str) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM information_schema.columns
              WHERE table_schema = current_schema()
                AND table_name = $1
                AND column_name = $2
         )",
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await?)
}

async fn nullable_count(
    pool: &PgPool,
    table: &str,
    column: &str,
    exists: bool,
) -> anyhow::Result<Option<i64>> {
    if !exists {
        return Ok(None);
    }

    // Table and column names come exclusively from constants in this binary.
    let sql = format!("SELECT count(*)::bigint FROM {table} WHERE {column} IS NULL");
    Ok(Some(scalar_count(pool, &sql).await?))
}

async fn scalar_count(pool: &PgPool, sql: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(AssertSqlSafe(sql))
        .fetch_one(pool)
        .await?)
}

async fn stream_categories(pool: &PgPool) -> anyhow::Result<BTreeMap<String, i64>> {
    let rows = sqlx::query(
        "SELECT stream_category, count(*)::bigint AS stream_count
           FROM es_streams
          GROUP BY stream_category
          ORDER BY stream_category",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| (row.get("stream_category"), row.get("stream_count")))
        .collect())
}

async fn projection_coverage(pool: &PgPool) -> anyhow::Result<BTreeMap<String, i64>> {
    let checks = [
        (
            "app",
            "SELECT count(*)::bigint
               FROM apps projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'app-' || projection.organisation_id::text || '/' || projection.name
              )",
        ),
        (
            "component",
            "SELECT count(*)::bigint
               FROM components projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'component-' || projection.organisation || '/' || projection.name
              )",
        ),
        (
            "destination",
            "SELECT count(*)::bigint
               FROM destinations projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'destination-' || projection.organisation || '/' || projection.name
              )",
        ),
        (
            "device_grant",
            "SELECT count(*)::bigint
               FROM device_login_grants projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'device_grant-' || projection.device_code_hash
              )",
        ),
        (
            "policy",
            "SELECT count(*)::bigint
               FROM policies projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'policy-' || projection.project_id::text || '/' || projection.name
              )",
        ),
        (
            "trigger",
            "SELECT count(*)::bigint
               FROM triggers projection
              WHERE NOT EXISTS (
                    SELECT 1 FROM es_streams stream
                     WHERE stream.stream_id = 'trigger-' || projection.project_id::text || '/' || projection.name
              )",
        ),
    ];

    let mut result = BTreeMap::new();
    for (category, sql) in checks {
        result.insert(category.to_string(), scalar_count(pool, sql).await?);
    }
    Ok(result)
}

async fn sequence_audit(pool: &PgPool) -> anyhow::Result<SequenceAudit> {
    let row = sqlx::query(
        "SELECT last_value::bigint AS last_value, is_called
           FROM es_events_global_position_seq",
    )
    .fetch_one(pool)
    .await?;
    let last_value: i64 = row.get("last_value");
    let is_called: bool = row.get("is_called");
    let maximum_global_position: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(global_position), 0)::bigint FROM es_events")
            .fetch_one(pool)
            .await?;

    let next_value = if is_called {
        last_value.saturating_add(1)
    } else {
        last_value
    };

    Ok(SequenceAudit {
        last_value,
        is_called,
        maximum_global_position,
        safe_for_next_insert: next_value > maximum_global_position,
    })
}

async fn decode_events(pool: &PgPool, max_error_samples: usize) -> anyhow::Result<DecodeAudit> {
    let mut rows = sqlx::query(
        "SELECT event.global_position,
                event.stream_id,
                stream.stream_category,
                event.event_type,
                event.data
           FROM es_events event
           JOIN es_streams stream USING (stream_id)
          ORDER BY event.global_position",
    )
    .fetch(pool);

    let mut checked = 0;
    let mut failures = 0;
    let mut samples = Vec::new();

    while let Some(row) = rows.try_next().await? {
        checked += 1;
        let global_position: i64 = row.get("global_position");
        let stream_id: String = row.get("stream_id");
        let stream_category: String = row.get("stream_category");
        let stored_event_type: String = row.get("event_type");
        let data: Value = row.get("data");

        let result = decode_event_type(&stream_category, data).and_then(|decoded_event_type| {
            if decoded_event_type == stored_event_type {
                Ok(())
            } else {
                Err(format!(
                    "payload resolves to event type {decoded_event_type:?}"
                ))
            }
        });

        if let Err(error) = result {
            failures += 1;
            if samples.len() < max_error_samples {
                samples.push(DecodeFailure {
                    global_position,
                    stream_id,
                    stream_category,
                    stored_event_type,
                    error,
                });
            }
        }
    }

    Ok(DecodeAudit {
        checked,
        failures,
        samples,
    })
}

fn decode_event_type(category: &str, data: Value) -> Result<String, String> {
    macro_rules! decode {
        ($event:ty) => {{
            let event: $event = serde_json::from_value(data).map_err(|error| error.to_string())?;
            Ok(event.event_type().to_string())
        }};
    }

    match category {
        "app" => decode!(AppEvent),
        "component" => decode!(ComponentEvent),
        "destination" => decode!(DestinationEvent),
        "device_grant" => decode!(DeviceGrantEvent),
        "policy" => decode!(PolicyEvent),
        "trigger" => decode!(TriggerEvent),
        other => Err(format!(
            "unsupported stream category {other:?}; supported categories are {}",
            SUPPORTED_CATEGORIES.join(", ")
        )),
    }
}
