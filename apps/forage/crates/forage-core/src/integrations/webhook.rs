use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The JSON payload delivered to webhook integrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    pub event: String,
    pub timestamp: String,
    pub organisation: String,
    pub project: String,
    pub notification_id: String,
    pub title: String,
    pub body: String,
    pub release: Option<ReleasePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePayload {
    pub slug: String,
    pub artifact_id: String,
    pub destination: String,
    pub environment: String,
    pub source_username: String,
    pub commit_sha: String,
    pub commit_branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Compute HMAC-SHA256 signature for a webhook payload.
/// Returns hex-encoded signature prefixed with "sha256=".
pub fn sign_payload(body: &[u8], secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    format!("sha256={}", hex_encode(&result))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_payload_produces_hex_signature() {
        let sig = sign_payload(b"hello world", "my-secret");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), 7 + 64); // "sha256=" + 64 hex chars
    }

    #[test]
    fn sign_payload_deterministic() {
        let a = sign_payload(b"test body", "key");
        let b = sign_payload(b"test body", "key");
        assert_eq!(a, b);
    }

    #[test]
    fn sign_payload_different_keys_differ() {
        let a = sign_payload(b"body", "key1");
        let b = sign_payload(b"body", "key2");
        assert_ne!(a, b);
    }

    #[test]
    fn webhook_payload_serializes() {
        let payload = WebhookPayload {
            event: "release_failed".into(),
            timestamp: "2026-03-09T14:30:00Z".into(),
            organisation: "test-org".into(),
            project: "my-project".into(),
            notification_id: "notif-123".into(),
            title: "Release failed".into(),
            body: "Container health check timeout".into(),
            release: Some(ReleasePayload {
                slug: "test-release".into(),
                artifact_id: "art_123".into(),
                destination: "prod-eu".into(),
                environment: "production".into(),
                source_username: "alice".into(),
                commit_sha: "abc1234".into(),
                commit_branch: "main".into(),
                error_message: Some("timeout".into()),
            }),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("release_failed"));
        assert!(json.contains("prod-eu"));
    }

    #[test]
    fn webhook_payload_without_release() {
        let payload = WebhookPayload {
            event: "release_annotated".into(),
            timestamp: "2026-03-09T14:30:00Z".into(),
            organisation: "test-org".into(),
            project: "my-project".into(),
            notification_id: "notif-456".into(),
            title: "Annotated".into(),
            body: "A note".into(),
            release: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["release"].is_null());
    }
}
