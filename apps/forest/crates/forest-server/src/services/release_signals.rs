use anyhow::Context;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRow {
    pub name: String,
    pub status: String,
    pub detail: String,
    pub destination_name: String,
    pub environment: String,
    pub reported_by: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
}

pub const VALID_STATUSES: [&str; 5] =
    ["HEALTHY", "PROGRESSING", "DEGRADED", "UNHEALTHY", "MISSING"];

pub fn is_valid_status(status: &str) -> bool {
    VALID_STATUSES.contains(&status)
}

#[allow(clippy::too_many_arguments)]
pub async fn report(
    db: &PgPool,
    nats: &async_nats::Client,
    release_intent_id: Uuid,
    release_id: Uuid,
    organisation: &str,
    project: &str,
    signal: &SignalRow,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO release_signals
            (release_intent_id, release_id, organisation, project,
             destination_name, environment, name, status, detail,
             reported_by, observed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (release_intent_id, destination_name, name)
        DO UPDATE SET
            release_id = EXCLUDED.release_id,
            status = EXCLUDED.status,
            detail = EXCLUDED.detail,
            reported_by = EXCLUDED.reported_by,
            observed_at = EXCLUDED.observed_at,
            updated_at = now()
        "#,
    )
    .bind(release_intent_id)
    .bind(release_id)
    .bind(organisation)
    .bind(project)
    .bind(&signal.destination_name)
    .bind(&signal.environment)
    .bind(&signal.name)
    .bind(&signal.status)
    .bind(&signal.detail)
    .bind(&signal.reported_by)
    .bind(signal.observed_at)
    .execute(db)
    .await
    .context("report release signal")?;

    let subject = format!("forest.release.signal.{release_intent_id}");
    let payload = serde_json::json!({
        "release_intent_id": release_intent_id.to_string(),
        "destination": signal.destination_name,
        "environment": signal.environment,
        "name": signal.name,
        "status": signal.status,
        "detail": signal.detail,
    });

    let _ = nats.publish(subject, payload.to_string().into()).await;

    let _ = nats
        .publish(
            "forest.intent.evaluate",
            release_intent_id.to_string().into(),
        )
        .await;

    Ok(())
}

pub async fn list_for_intent(
    db: &PgPool,
    release_intent_id: Uuid,
) -> anyhow::Result<Vec<SignalRow>> {
    let rows = sqlx::query(
        r#"
        SELECT name, status, detail, destination_name, environment,
               reported_by, observed_at
        FROM release_signals
        WHERE release_intent_id = $1
        ORDER BY observed_at DESC, name ASC
        "#,
    )
    .bind(release_intent_id)
    .fetch_all(db)
    .await
    .context("list release signals")?;

    Ok(rows
        .into_iter()
        .map(|r| SignalRow {
            name: r.get("name"),
            status: r.get("status"),
            detail: r.get("detail"),
            destination_name: r.get("destination_name"),
            environment: r.get("environment"),
            reported_by: r.get("reported_by"),
            observed_at: r.get("observed_at"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_health_status_is_accepted() {
        for status in VALID_STATUSES {
            assert!(is_valid_status(status), "{status} should be valid");
        }
    }

    #[test]
    fn an_unknown_status_is_rejected() {
        for status in ["", "healthy", "OK", "UNSPECIFIED", "HEALTHY "] {
            assert!(!is_valid_status(status), "{status:?} should be rejected");
        }
    }
}
