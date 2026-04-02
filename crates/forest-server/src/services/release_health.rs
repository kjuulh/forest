use anyhow::Context;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Upsert a health observation for a release + destination.
/// Publishes a NATS message for streaming subscribers.
pub async fn upsert_observation(
    db: &PgPool,
    nats: &async_nats::Client,
    release_intent_id: Uuid,
    release_id: Uuid,
    destination_name: &str,
    environment: &str,
    organisation: &str,
    project: &str,
    observation_json: &serde_json::Value,
    status: &str,
    message: &str,
    observed_at: &chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO release_health_observations
            (release_intent_id, release_id, destination_name, environment,
             organisation, project, observation, status, message, observed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (release_intent_id, destination_name)
        DO UPDATE SET
            release_id = EXCLUDED.release_id,
            observation = EXCLUDED.observation,
            status = EXCLUDED.status,
            message = EXCLUDED.message,
            observed_at = EXCLUDED.observed_at,
            updated_at = now()
        "#,
    )
    .bind(release_intent_id)
    .bind(release_id)
    .bind(destination_name)
    .bind(environment)
    .bind(organisation)
    .bind(project)
    .bind(observation_json)
    .bind(status)
    .bind(message)
    .bind(observed_at)
    .execute(db)
    .await
    .context("upsert health observation")?;

    // Publish NATS event for streaming subscribers
    let nats_subject = format!("forest.release.health.{}", release_intent_id);
    let payload = serde_json::json!({
        "release_intent_id": release_intent_id.to_string(),
        "destination": destination_name,
        "environment": environment,
        "status": status,
        "message": message,
    });

    let _ = nats
        .publish(nats_subject, payload.to_string().into())
        .await;

    Ok(())
}

/// Get all health observations for a release intent.
pub async fn get_observations_for_intent(
    db: &PgPool,
    release_intent_id: Uuid,
) -> anyhow::Result<Vec<HealthObservationRow>> {
    let rows = sqlx::query(
        r#"
        SELECT destination_name, environment, observation, status, message, observed_at
        FROM release_health_observations
        WHERE release_intent_id = $1
        ORDER BY destination_name
        "#,
    )
    .bind(release_intent_id)
    .fetch_all(db)
    .await
    .context("get health observations")?;

    Ok(rows
        .into_iter()
        .map(|row| HealthObservationRow {
            destination_name: row.get("destination_name"),
            environment: row.get("environment"),
            observation: row.get("observation"),
            status: row.get("status"),
            message: row.get("message"),
            observed_at: row.get("observed_at"),
        })
        .collect())
}

/// Seed a PENDING health observation when a release is created.
/// This ensures the WatchReleaseHealth stream immediately has data.
pub async fn seed_pending(
    db: &PgPool,
    nats: &async_nats::Client,
    release_intent_id: Uuid,
    release_id: Uuid,
    destination_name: &str,
    environment: &str,
    organisation: &str,
    project: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let observation_json = serde_json::json!({
        "resources": [],
        "observed_at": now.to_rfc3339(),
        "status": "PENDING",
        "message": "waiting for health agent to report",
    });

    upsert_observation(
        db,
        nats,
        release_intent_id,
        release_id,
        destination_name,
        environment,
        organisation,
        project,
        &observation_json,
        "PENDING",
        "waiting for health agent to report",
        &now,
    )
    .await
}

pub struct HealthObservationRow {
    pub destination_name: String,
    pub environment: String,
    pub observation: serde_json::Value,
    pub status: String,
    pub message: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

/// Compute the aggregate health status from per-destination statuses.
pub fn aggregate_status(rows: &[HealthObservationRow]) -> &'static str {
    if rows.is_empty() {
        return "UNSPECIFIED";
    }

    let all_healthy = rows.iter().all(|r| r.status == "HEALTHY");
    if all_healthy {
        return "HEALTHY";
    }

    let any_unhealthy = rows.iter().any(|r| r.status == "UNHEALTHY");
    if any_unhealthy {
        return "UNHEALTHY";
    }

    let any_degraded = rows.iter().any(|r| r.status == "DEGRADED");
    if any_degraded {
        return "DEGRADED";
    }

    "PROGRESSING"
}
