use super::release_event_store::{EventPayload, ReleaseEventStore, ReleaseEventType, ReleaseState};
use anyhow::Context;

pub fn applies_to(type_organisation: &str, type_name: &str) -> bool {
    type_organisation == "forest" && type_name == "generic"
}

pub fn overtaken_reason(slug: &str, destination: &str) -> String {
    format!(
        "superseded by release {slug}: a newer release is already deployed to \
         {destination}, and an older one never deploys over it"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overtaken {
    No,
    WaitsFor(String),
    Superseded,
}

struct Newer {
    slug: String,
    deployed: bool,
    queued_deploy: Option<uuid::Uuid>,
}

async fn newer_deploy(
    db: &sqlx::PgPool,
    release: &ReleaseState,
    same_branch: Option<&str>,
) -> anyhow::Result<Option<Newer>> {
    let newer = sqlx::query!(
        r#"SELECT ri.id, a.slug as "slug?",
                  EXISTS (
                      SELECT 1 FROM release_states rs
                      WHERE rs.release_intent_id = ri.id
                        AND rs.destination_id = $3
                        AND rs.mode <> 'plan'
                        AND rs.status = 'SUCCEEDED'
                  ) as "deployed!",
                  (
                      SELECT rs.release_id FROM release_states rs
                      WHERE rs.release_intent_id = ri.id
                        AND rs.destination_id = $3
                        AND rs.mode <> 'plan'
                        AND rs.status = 'QUEUED'
                      LIMIT 1
                  ) as "queued_deploy?"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.project_id = $1
             AND ri.id <> $2
             AND ri.created > (SELECT created FROM release_intents WHERE id = $2)
             AND ($4::text IS NULL OR a.ref->>'commit_branch' = $4)
             AND EXISTS (
                 SELECT 1 FROM release_states rs
                 WHERE rs.release_intent_id = ri.id
                   AND rs.destination_id = $3
                   AND rs.mode <> 'plan'
                   AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING', 'SUCCEEDED')
             )
           ORDER BY 3 DESC, ri.created DESC
           LIMIT 1"#,
        release.project_id,
        release.release_intent_id,
        release.destination_id,
        same_branch,
    )
    .fetch_optional(db)
    .await
    .context("look for a newer deploy to the same destination")?;
    Ok(newer.map(|n| Newer {
        slug: n.slug.unwrap_or_else(|| n.id.to_string()),
        deployed: n.deployed,
        queued_deploy: n.queued_deploy,
    }))
}

/// Decides a queued deploy against newer releases of the same destination. A
/// newer deploy that succeeded supersedes it (and `Superseded` means it was
/// retired and must not be dispatched); one still queued or running makes it
/// wait, so it still lands if that newer deploy fails. With `same_branch`, only
/// releases from that branch count.
pub async fn retire_if_overtaken(
    event_store: &ReleaseEventStore,
    nats: &async_nats::Client,
    release: &ReleaseState,
    destination: &str,
    same_branch: Option<&str>,
) -> anyhow::Result<Overtaken> {
    if release.mode == "plan" {
        return Ok(Overtaken::No);
    }
    let Some(newer) = newer_deploy(&event_store.db, release, same_branch).await? else {
        return Ok(Overtaken::No);
    };
    if !newer.deployed {
        move_to_back_of_sweep(&event_store.db, release.release_id).await?;
        if let Some(queued) = newer.queued_deploy {
            let _ = nats
                .publish("forest.release.queued", queued.to_string().into())
                .await;
        }
        return Ok(Overtaken::WaitsFor(newer.slug));
    }
    event_store
        .emit_event(
            release.release_id,
            ReleaseEventType::Superseded,
            EventPayload {
                reason: Some(overtaken_reason(&newer.slug, destination)),
                ..Default::default()
            },
            None,
        )
        .await
        .context("supersede the overtaken deploy")?;
    super::supersede::notify_superseded(event_store, nats, release.release_id).await;
    Ok(Overtaken::Superseded)
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

pub async fn move_to_back_of_sweep(
    db: &sqlx::PgPool,
    release_id: uuid::Uuid,
) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE release_states SET updated_at = now()
         WHERE release_id = $1 AND status = 'QUEUED'",
        release_id,
    )
    .execute(db)
    .await
    .context("move the waiting deploy to the back of the sweep")?;
    Ok(())
}
