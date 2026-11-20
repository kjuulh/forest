use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Signed Slack requests further than this from our clock are treated as replays.
pub const SLACK_MAX_SKEW_SECS: i64 = 300;

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn verify_hmac(secret: &str, message: &[u8], expected_hex: &str) -> bool {
    let Some(expected) = hex_decode(expected_hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(message);
    mac.verify_slice(&expected).is_ok()
}

/// Verifies `X-Slack-Signature` (`v0=<hex>`) over `v0:{timestamp}:{body}` and
/// refuses timestamps more than five minutes from `now_secs`.
pub fn verify_slack_signature(
    signing_secret: &str,
    timestamp: &str,
    body: &[u8],
    signature: &str,
    now_secs: i64,
) -> bool {
    if signing_secret.is_empty() {
        return false;
    }
    let Ok(ts) = timestamp.parse::<i64>() else {
        return false;
    };
    if (now_secs - ts).abs() > SLACK_MAX_SKEW_SECS {
        return false;
    }
    let Some(hex) = signature.strip_prefix("v0=") else {
        return false;
    };
    let mut message = format!("v0:{timestamp}:").into_bytes();
    message.extend_from_slice(body);
    verify_hmac(signing_secret, &message, hex)
}

/// Verifies GitHub's `X-Hub-Signature-256` (`sha256=<hex>`) over the raw body.
pub fn verify_github_signature(webhook_secret: &str, body: &[u8], signature: &str) -> bool {
    if webhook_secret.is_empty() {
        return false;
    }
    let Some(hex) = signature.strip_prefix("sha256=") else {
        return false;
    };
    verify_hmac(webhook_secret, body, hex)
}

pub fn sign_hex(secret: &str, message: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(message);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correctly_signed_fresh_slack_request_verifies() {
        let body = br#"{"type":"event_callback"}"#;
        let sig = format!(
            "v0={}",
            sign_hex("s3cret", b"v0:1000:{\"type\":\"event_callback\"}")
        );
        assert!(verify_slack_signature("s3cret", "1000", body, &sig, 1100));
    }

    #[test]
    fn a_tampered_or_replayed_slack_request_is_refused() {
        let sig = format!("v0={}", sign_hex("s3cret", b"v0:1000:{}"));
        assert!(!verify_slack_signature(
            "s3cret", "1000", b"{ }", &sig, 1000
        ));
        assert!(!verify_slack_signature(
            "s3cret",
            "1000",
            b"{}",
            &sig,
            1000 + 301
        ));
        assert!(!verify_slack_signature("other", "1000", b"{}", &sig, 1000));
        assert!(!verify_slack_signature("", "1000", b"{}", &sig, 1000));
        assert!(!verify_slack_signature(
            "s3cret", "1000", b"{}", "v1=00", 1000
        ));
    }

    #[test]
    fn github_signatures_verify_only_with_the_right_secret() {
        let sig = format!("sha256={}", sign_hex("hook", b"payload"));
        assert!(verify_github_signature("hook", b"payload", &sig));
        assert!(!verify_github_signature("hook", b"payload2", &sig));
        assert!(!verify_github_signature("nope", b"payload", &sig));
        assert!(!verify_github_signature("", b"payload", &sig));
        assert!(!verify_github_signature("hook", b"payload", "sha1=abc"));
    }
}
