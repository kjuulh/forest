use super::release_event_store::{EventPayload, ReleaseEventStore, ReleaseEventType, ReleaseState};
use anyhow::Context;

pub fn applies_to(type_organisation: &str, type_name: &str) -> bool {
    type_organisation == "forest" && type_name == "generic"
}

pub fn overtaken_reason(slug: &str, destination: &str) -> String {
    format!(
        "superseded by release {slug}: a newer release is already deployed or queued for \
         {destination}, and an older one never deploys over it"
    )
}

pub async fn newer_deploy(
    db: &sqlx::PgPool,
    release: &ReleaseState,
) -> anyhow::Result<Option<String>> {
    let newer = sqlx::query!(
        r#"SELECT ri.id, a.slug as "slug?"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.project_id = $1
             AND ri.id <> $2
             AND ri.created > (SELECT created FROM release_intents WHERE id = $2)
             AND EXISTS (
                 SELECT 1 FROM release_states rs
                 WHERE rs.release_intent_id = ri.id
                   AND rs.destination_id = $3
                   AND rs.mode <> 'plan'
                   AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING', 'SUCCEEDED')
             )
           ORDER BY ri.created DESC
           LIMIT 1"#,
        release.project_id,
        release.release_intent_id,
        release.destination_id,
    )
    .fetch_optional(db)
    .await
    .context("look for a newer deploy to the same destination")?;
    Ok(newer.map(|n| n.slug.unwrap_or_else(|| n.id.to_string())))
}

/// Retires a queued deploy that a newer release has overtaken for the same
/// destination. Returns true when the release was superseded and must not be
/// dispatched.
pub async fn retire_if_overtaken(
    event_store: &ReleaseEventStore,
    nats: &async_nats::Client,
    release: &ReleaseState,
    destination: &str,
) -> anyhow::Result<bool> {
    if release.mode == "plan" {
        return Ok(false);
    }
    let Some(slug) = newer_deploy(&event_store.db, release).await? else {
        return Ok(false);
    };
    event_store
        .emit_event(
            release.release_id,
            ReleaseEventType::Superseded,
            EventPayload {
                reason: Some(overtaken_reason(&slug, destination)),
                ..Default::default()
            },
            None,
        )
        .await
        .context("supersede the overtaken deploy")?;
    super::supersede::notify_superseded(event_store, nats, release.release_id).await;
    Ok(true)
}

pub async fn deploy_in_flight(
    db: &sqlx::PgPool,
    release: &ReleaseState,
) -> anyhow::Result<Option<String>> {
    if release.mode == "plan" {
        return Ok(None);
    }
    let running = sqlx::query!(
        r#"SELECT rs.release_intent_id, a.slug as "slug?"
           FROM release_states rs
           JOIN release_intents ri ON ri.id = rs.release_intent_id
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE rs.destination_id = $1
             AND rs.release_id <> $2
             AND rs.mode <> 'plan'
             AND rs.status IN ('ASSIGNED', 'RUNNING')
           LIMIT 1"#,
        release.destination_id,
        release.release_id,
    )
    .fetch_optional(db)
    .await
    .context("look for a deploy already running on the destination")?;
    Ok(running.map(|r| r.slug.unwrap_or_else(|| r.release_intent_id.to_string())))
}
