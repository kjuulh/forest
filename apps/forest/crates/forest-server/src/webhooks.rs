use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::{PgPool, Row};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct WebhookState {
    pub db: PgPool,
}

pub fn webhook_routes(state: WebhookState) -> Router {
    Router::new()
        .route(
            "/webhooks/flux/notifications/{organisation}/{destination_name}",
            post(handle_flux_notification),
        )
        .with_state(state)
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct FluxNotificationPayload {
    involved_object: FluxInvolvedObject,
    severity: String,
    message: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    metadata: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct FluxInvolvedObject {
    kind: String,
    name: String,
    namespace: String,
}

/// Verify HMAC-SHA256 signature from Flux generic-hmac provider.
/// Flux sends the header as `sha256=<hex-encoded HMAC>`.
fn verify_hmac_signature(secret: &[u8], body: &[u8], signature_header: &str) -> bool {
    let Some(hex_sig) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

async fn handle_flux_notification(
    State(state): State<WebhookState>,
    Path((organisation, destination_name)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // 1. Look up destination by (org, name) to get webhook_secret from metadata
    let dest = match lookup_destination_with_secret(&state.db, &organisation, &destination_name).await {
        Ok(Some(dest)) => dest,
        Ok(None) => {
            tracing::warn!(
                destination = %destination_name,
                "flux webhook: destination not found"
            );
            return StatusCode::NOT_FOUND;
        }
        Err(e) => {
            tracing::error!(
                destination = %destination_name,
                error = %e,
                "flux webhook: failed to look up destination"
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    let Some(webhook_secret) = &dest.webhook_secret else {
        tracing::warn!(
            destination = %destination_name,
            "flux webhook: destination has no webhook_secret configured"
        );
        return StatusCode::FORBIDDEN;
    };

    // 2. Verify HMAC signature
    let signature = match headers.get("x-signature") {
        Some(val) => val.to_str().unwrap_or(""),
        None => {
            tracing::warn!(
                destination = %destination_name,
                "flux webhook: missing X-Signature header"
            );
            return StatusCode::UNAUTHORIZED;
        }
    };

    if !verify_hmac_signature(webhook_secret.as_bytes(), &body, signature) {
        tracing::warn!(
            destination = %destination_name,
            "flux webhook: HMAC signature mismatch"
        );
        return StatusCode::UNAUTHORIZED;
    }

    // 3. Parse payload
    let payload: FluxNotificationPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                destination = %destination_name,
                error = %e,
                "flux webhook: failed to parse payload"
            );
            return StatusCode::BAD_REQUEST;
        }
    };

    tracing::info!(
        destination = %destination_name,
        severity = %payload.severity,
        reason = %payload.reason,
        kind = %payload.involved_object.kind,
        name = %payload.involved_object.name,
        message = %payload.message,
        "flux webhook: received notification"
    );

    // 4. Determine success or failure based on Flux event semantics
    let is_success = payload.severity == "info"
        && matches!(
            payload.reason.as_str(),
            "Succeeded" | "ReconciliationSucceeded"
        );
    let is_failure = payload.severity == "error";

    if !is_success && !is_failure {
        // Informational event we don't need to act on (e.g. progressing)
        tracing::debug!(
            destination = %destination_name,
            reason = %payload.reason,
            "flux webhook: ignoring non-terminal event"
        );
        return StatusCode::OK;
    }

    // 5. Find the most recent RUNNING release for this destination
    let running_release = match get_latest_running_release(&state.db, &dest.destination_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            tracing::debug!(
                destination = %destination_name,
                "flux webhook: no RUNNING release found, ignoring"
            );
            return StatusCode::OK;
        }
        Err(e) => {
            tracing::error!(
                destination = %destination_name,
                error = %e,
                "flux webhook: failed to query running release"
            );
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    // 6. Transition release state
    let (event_type, error_message) = if is_success {
        ("release.succeeded", None)
    } else {
        (
            "release.failed",
            Some(format!("Flux reconciliation failed: {}", payload.message)),
        )
    };

    if let Err(e) = transition_release(
        &state.db,
        &running_release.release_id,
        event_type,
        error_message.as_deref(),
    )
    .await
    {
        tracing::error!(
            destination = %destination_name,
            release_id = %running_release.release_id,
            error = %e,
            "flux webhook: failed to transition release"
        );
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    tracing::info!(
        destination = %destination_name,
        release_id = %running_release.release_id,
        status = event_type,
        "flux webhook: release state transitioned"
    );

    StatusCode::OK
}

// ====== DB HELPERS ======

struct DestinationWithSecret {
    destination_id: uuid::Uuid,
    webhook_secret: Option<String>,
}

async fn lookup_destination_with_secret(
    db: &PgPool,
    organisation: &str,
    name: &str,
) -> anyhow::Result<Option<DestinationWithSecret>> {
    let rec = sqlx::query(
        "SELECT id, metadata FROM destinations
         WHERE organisation = $1 AND name = $2
         LIMIT 1",
    )
    .bind(organisation)
    .bind(name)
    .fetch_optional(db)
    .await?;

    let Some(rec) = rec else { return Ok(None) };

    let id: uuid::Uuid = rec.get("id");
    let metadata_json: serde_json::Value = rec.get("metadata");
    let metadata: std::collections::HashMap<String, String> =
        serde_json::from_value(metadata_json).unwrap_or_default();

    Ok(Some(DestinationWithSecret {
        destination_id: id,
        webhook_secret: metadata.get("webhook_secret").cloned().filter(|s| !s.is_empty()),
    }))
}

struct RunningRelease {
    release_id: uuid::Uuid,
}

async fn get_latest_running_release(
    db: &PgPool,
    destination_id: &uuid::Uuid,
) -> anyhow::Result<Option<RunningRelease>> {
    let rec = sqlx::query(
        "SELECT release_id FROM release_states
         WHERE destination_id = $1 AND status = 'RUNNING'
         ORDER BY queued_at DESC
         LIMIT 1",
    )
    .bind(destination_id)
    .fetch_optional(db)
    .await?;

    Ok(rec.map(|r| RunningRelease {
        release_id: r.get("release_id"),
    }))
}

async fn transition_release(
    db: &PgPool,
    release_id: &uuid::Uuid,
    event_type: &str,
    error_message: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;

    let target_status = match event_type {
        "release.succeeded" => "SUCCEEDED",
        "release.failed" => "FAILED",
        _ => anyhow::bail!("unexpected event type: {event_type}"),
    };

    // Guard: only transition from RUNNING
    let updated = sqlx::query(
        "UPDATE release_states
         SET status = $2, error_message = $3,
             completed_at = now(), updated_at = now()
         WHERE release_id = $1 AND status = 'RUNNING'",
    )
    .bind(release_id)
    .bind(target_status)
    .bind(error_message)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        // Release is no longer RUNNING — another transition beat us
        tracing::debug!(
            release_id = %release_id,
            "flux webhook: release already transitioned, skipping"
        );
        tx.rollback().await?;
        return Ok(());
    }

    // Insert event (actor_id is NULL for system-initiated webhook events)
    let payload = serde_json::json!({
        "error_message": error_message,
        "reason": "flux_webhook_notification",
    });
    let actor_id: Option<uuid::Uuid> = None;
    sqlx::query(
        "INSERT INTO release_events (
            release_id, event_type, payload, actor_id, actor_type
        ) VALUES ($1, $2, $3, $4, 'system')",
    )
    .bind(release_id)
    .bind(event_type)
    .bind(&payload)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signature(secret: &[u8], body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn test_verify_hmac_signature_valid() {
        let secret = b"my-secret-token";
        let body = b"hello world";
        let sig = make_signature(secret, body);
        assert!(verify_hmac_signature(secret, body, &sig));
    }

    #[test]
    fn test_verify_hmac_signature_invalid() {
        let secret = b"my-secret-token";
        let body = b"hello world";
        assert!(!verify_hmac_signature(secret, body, "invalid-signature"));
    }

    #[test]
    fn test_verify_hmac_signature_missing_prefix() {
        let secret = b"my-secret-token";
        let body = b"hello world";
        let mut mac = HmacSha256::new_from_slice(secret).unwrap();
        mac.update(body);
        let bare_hex = hex::encode(mac.finalize().into_bytes());
        // Without sha256= prefix, should fail
        assert!(!verify_hmac_signature(secret, body, &bare_hex));
    }

    #[test]
    fn test_verify_hmac_signature_wrong_secret() {
        let secret = b"my-secret-token";
        let wrong_secret = b"wrong-secret";
        let body = b"hello world";
        let sig = make_signature(wrong_secret, body);
        assert!(!verify_hmac_signature(secret, body, &sig));
    }

    #[test]
    fn test_verify_hmac_signature_empty_body() {
        let secret = b"secret";
        let body = b"";
        let sig = make_signature(secret, body);
        assert!(verify_hmac_signature(secret, body, &sig));
    }

    #[test]
    fn test_flux_payload_deserialization() {
        let json = r#"{
            "involvedObject": {
                "kind": "Kustomization",
                "name": "my-app",
                "namespace": "flux-system"
            },
            "severity": "info",
            "message": "Reconciliation finished in 5s",
            "reason": "ReconciliationSucceeded",
            "timestamp": "2026-03-31T10:00:00Z"
        }"#;

        let payload: FluxNotificationPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.severity, "info");
        assert_eq!(payload.reason, "ReconciliationSucceeded");
        assert_eq!(payload.involved_object.kind, "Kustomization");
        assert_eq!(payload.involved_object.name, "my-app");
    }

    #[test]
    fn test_flux_payload_deserialization_minimal() {
        let json = r#"{
            "involvedObject": {
                "kind": "GitRepository",
                "name": "flux-system",
                "namespace": "flux-system"
            },
            "severity": "error",
            "message": "reconciliation failed"
        }"#;

        let payload: FluxNotificationPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.severity, "error");
        assert_eq!(payload.reason, ""); // default
        assert!(payload.metadata.is_none());
        assert!(payload.timestamp.is_none());
    }
}
