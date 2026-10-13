//! Credentials for the `forest/terraform@1` HTTP state backend.
//!
//! Before this, the server generated a random password per state id, kept it in
//! an in-memory map, handed it to every run as `TF_HTTP_PASSWORD` — and never
//! compared it against anything. Every route of the state server was open to
//! anything that could reach its port (understory-io/forest#281).
//!
//! A credential is now a signed statement of *which state*, *what it may do*,
//! *until when*, and *on whose behalf*:
//!
//! ```text
//! fst1.<scope>.<expires unix>.<principal, base64url>.<hmac-sha256, base64url>
//! ```
//!
//! The MAC covers all four fields **and the state id**, so a credential minted
//! for one state is worthless against another even though the state id is not
//! written into the token itself. Verification is stateless: it needs only the
//! key, so it survives a forest-server restart and holds across replicas —
//! neither of which the in-memory map could promise once it was actually
//! checked.
//!
//! The key comes from its own secret, `FOREST_TERRAFORM_STATE_SIGNING_KEY`, so
//! rotating the access-token secret does not revoke state credentials and
//! rotating this one does not log anybody out. Unset, it falls back to a key
//! derived from the access-token secret (the server warns at startup), so a
//! deploy needs no configuration change. Either way the secret is run through
//! HMAC with a fixed label before use, so the bytes that sign state credentials
//! are never the bytes that sign anything else.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const VERSION: &str = "fst1";
const KEY_LABEL: &[u8] = b"forest/terraform-v1/state-credentials/v1";

/// Longest life an operator may ask for. A runner's credential is minted per
/// run and sized to the run; see [`RUNNER_TTL`].
pub const MAX_OPERATOR_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// A run is init + plan or init + apply. Applies that wait on ACM validation
/// run long, and a credential that expires mid-apply strands the state lock
/// with the apply half done — so this is generous rather than tight.
pub const RUNNER_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// GET the state. No lock, no write, no unlock.
    ReadOnly,
    /// Everything terraform's HTTP backend does: read, lock, write, unlock.
    ReadWrite,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::ReadOnly => "ro",
            Scope::ReadWrite => "rw",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "ro" => Some(Scope::ReadOnly),
            "rw" => Some(Scope::ReadWrite),
            _ => None,
        }
    }
}

/// What a valid credential says about the request carrying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub scope: Scope,
    /// Who the credential was minted for — `runner:<destination>` for a
    /// release, `user:<id>` for an operator. Logged on every request, which is
    /// the audit trail for who touched a state.
    pub principal: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Malformed,
    BadSignature,
    Expired,
}

#[derive(Clone)]
pub struct CredentialKey {
    key: Vec<u8>,
}

impl std::fmt::Debug for CredentialKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialKey(..)")
    }
}

impl CredentialKey {
    /// The key a server signs with: its dedicated signing secret when
    /// configured, the access-token secret otherwise.
    pub fn from_secrets(dedicated: Option<&[u8]>, access_token_secret: &[u8]) -> Self {
        Self::derive(dedicated.unwrap_or(access_token_secret))
    }

    /// Derive the state-credential key from a secret.
    pub fn derive(access_token_secret: &[u8]) -> Self {
        let mut mac =
            HmacSha256::new_from_slice(access_token_secret).expect("hmac accepts any key length");
        mac.update(KEY_LABEL);
        Self {
            key: mac.finalize().into_bytes().to_vec(),
        }
    }

    pub fn issue(&self, state_id: &str, scope: Scope, principal: &str, ttl: Duration) -> String {
        self.issue_with_expiry(state_id, scope, principal, ttl).0
    }

    /// As [`issue`](Self::issue), also returning the expiry (unix seconds).
    pub fn issue_with_expiry(
        &self,
        state_id: &str,
        scope: Scope,
        principal: &str,
        ttl: Duration,
    ) -> (String, u64) {
        let expires_at = now_unix().saturating_add(ttl.as_secs());
        (
            self.issue_expiring_at(state_id, scope, principal, expires_at),
            expires_at,
        )
    }

    fn issue_expiring_at(
        &self,
        state_id: &str,
        scope: Scope,
        principal: &str,
        expires_at: u64,
    ) -> String {
        let principal_b64 = URL_SAFE_NO_PAD.encode(principal.as_bytes());
        let mac = self.mac(state_id, scope.as_str(), expires_at, &principal_b64);
        format!(
            "{VERSION}.{}.{expires_at}.{principal_b64}.{}",
            scope.as_str(),
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        )
    }

    /// Check a credential against the state id the request is for.
    ///
    /// The signature comparison is `Mac::verify_slice`, which is constant-time.
    pub fn verify(&self, state_id: &str, credential: &str) -> Result<Verified, Rejection> {
        self.verify_at(state_id, credential, now_unix())
    }

    fn verify_at(&self, state_id: &str, credential: &str, now: u64) -> Result<Verified, Rejection> {
        let mut parts = credential.split('.');
        let (Some(version), Some(scope), Some(expires), Some(principal_b64), Some(sig), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(Rejection::Malformed);
        };
        if version != VERSION {
            return Err(Rejection::Malformed);
        }
        let scope_parsed = Scope::parse(scope).ok_or(Rejection::Malformed)?;
        let expires_at: u64 = expires.parse().map_err(|_| Rejection::Malformed)?;
        let sig = URL_SAFE_NO_PAD
            .decode(sig)
            .map_err(|_| Rejection::Malformed)?;

        // Signature before expiry, so an expired-but-forged credential is
        // reported as forged.
        self.mac(state_id, scope, expires_at, principal_b64)
            .verify_slice(&sig)
            .map_err(|_| Rejection::BadSignature)?;

        if now >= expires_at {
            return Err(Rejection::Expired);
        }

        let principal = URL_SAFE_NO_PAD
            .decode(principal_b64)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(Rejection::Malformed)?;

        Ok(Verified {
            scope: scope_parsed,
            principal,
            expires_at,
        })
    }

    fn mac(&self, state_id: &str, scope: &str, expires_at: u64, principal_b64: &str) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("hmac accepts any key length");
        // Length-prefixed rather than joined with a separator, so no choice of
        // state id or principal can make two different tuples sign the same.
        for field in [
            VERSION.as_bytes(),
            scope.as_bytes(),
            expires_at.to_string().as_bytes(),
            principal_b64.as_bytes(),
            state_id.as_bytes(),
        ] {
            mac.update(&(field.len() as u64).to_be_bytes());
            mac.update(field);
        }
        mac
    }
}

/// Pull the password out of an `Authorization: Basic …` header. The username
/// is ignored: terraform sends `TF_HTTP_USERNAME`, the runner sets it to
/// `forest-terraform-v1`, and everything that matters is in the password.
pub fn basic_auth_password(header: Option<&http::HeaderValue>) -> Option<String> {
    let value = header?.to_str().ok()?;
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (_, password) = decoded.split_once(':')?;
    Some(password.to_string())
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> CredentialKey {
        CredentialKey::derive(b"test-access-token-secret-key-32b")
    }

    #[test]
    fn a_credential_verifies_for_the_state_it_was_minted_for() {
        let cred = key().issue(
            "dev.p1",
            Scope::ReadWrite,
            "runner:dev/eu-west-1/core",
            Duration::from_secs(60),
        );
        let verified = key().verify("dev.p1", &cred).expect("valid");
        assert_eq!(verified.scope, Scope::ReadWrite);
        assert_eq!(verified.principal, "runner:dev/eu-west-1/core");
    }

    #[test]
    fn a_credential_for_one_state_is_refused_for_another() {
        // The whole point of #281: holding a credential for your own state
        // must not open anyone else's.
        let cred = key().issue(
            "dev.p1",
            Scope::ReadWrite,
            "user:a",
            Duration::from_secs(60),
        );
        assert_eq!(key().verify("dev.p2", &cred), Err(Rejection::BadSignature));
        assert_eq!(key().verify("prod.p1", &cred), Err(Rejection::BadSignature));
    }

    #[test]
    fn a_credential_from_another_key_is_refused() {
        let other = CredentialKey::derive(b"some-other-server-secret-key-32b");
        let cred = other.issue(
            "dev.p1",
            Scope::ReadWrite,
            "user:a",
            Duration::from_secs(60),
        );
        assert_eq!(key().verify("dev.p1", &cred), Err(Rejection::BadSignature));
    }

    #[test]
    fn widening_the_scope_breaks_the_signature() {
        let cred = key().issue("dev.p1", Scope::ReadOnly, "user:a", Duration::from_secs(60));
        let widened = cred.replacen(".ro.", ".rw.", 1);
        assert_eq!(
            key().verify("dev.p1", &widened),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn extending_the_expiry_breaks_the_signature() {
        let cred = key().issue_expiring_at("dev.p1", Scope::ReadWrite, "user:a", 1_000);
        let extended = cred.replacen(".1000.", ".9999999999.", 1);
        assert_eq!(
            key().verify_at("dev.p1", &extended, 10),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn changing_the_principal_breaks_the_signature() {
        // The principal is the audit trail; it must not be rewritable.
        let cred = key().issue(
            "dev.p1",
            Scope::ReadWrite,
            "user:a",
            Duration::from_secs(60),
        );
        let parts: Vec<&str> = cred.split('.').collect();
        let forged = format!(
            "{}.{}.{}.{}.{}",
            parts[0],
            parts[1],
            parts[2],
            URL_SAFE_NO_PAD.encode("user:b"),
            parts[4]
        );
        assert_eq!(
            key().verify("dev.p1", &forged),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn an_expired_credential_is_refused() {
        let cred = key().issue_expiring_at("dev.p1", Scope::ReadWrite, "user:a", 1_000);
        assert!(key().verify_at("dev.p1", &cred, 999).is_ok());
        assert_eq!(
            key().verify_at("dev.p1", &cred, 1_000),
            Err(Rejection::Expired)
        );
    }

    #[test]
    fn the_old_random_uuid_passwords_are_refused() {
        // What every run was handed before this change.
        assert_eq!(
            key().verify("dev.p1", "3f0e4c6a-7d1b-4f5e-9a2c-1b2c3d4e5f60"),
            Err(Rejection::Malformed)
        );
        assert_eq!(key().verify("dev.p1", ""), Err(Rejection::Malformed));
        assert_eq!(
            key().verify("dev.p1", "fst1.rw.1.a.b.c"),
            Err(Rejection::Malformed)
        );
    }

    #[test]
    fn length_prefixing_keeps_field_boundaries_unambiguous() {
        // With naive concatenation, ("dev.p", principal "1…") and
        // ("dev.p1", principal "…") could collide. Length prefixes rule it out.
        let a = key().issue_expiring_at("ab", Scope::ReadWrite, "c", 10);
        let b = key().issue_expiring_at("a", Scope::ReadWrite, "bc", 10);
        assert_ne!(a.rsplit('.').next(), b.rsplit('.').next());
    }

    #[test]
    fn a_dedicated_signing_key_is_independent_of_the_access_token_secret() {
        let dedicated = b"a-dedicated-state-signing-key-32b".as_slice();
        let before =
            CredentialKey::from_secrets(Some(dedicated), b"access-secret-one-xxxxxxxxxxxxxxx");
        let rotated =
            CredentialKey::from_secrets(Some(dedicated), b"access-secret-two-xxxxxxxxxxxxxxx");
        let cred = before.issue(
            "dev.p1",
            Scope::ReadWrite,
            "user:a",
            Duration::from_secs(60),
        );

        // Rotating the access-token secret leaves state credentials valid...
        assert!(rotated.verify("dev.p1", &cred).is_ok());
        // ...and the fallback key is a different key.
        let fallback = CredentialKey::from_secrets(None, b"access-secret-one-xxxxxxxxxxxxxxx");
        assert_eq!(
            fallback.verify("dev.p1", &cred),
            Err(Rejection::BadSignature)
        );
    }

    #[test]
    fn basic_auth_is_parsed_and_the_username_ignored() {
        let header = http::HeaderValue::from_str(&format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("forest-terraform-v1:fst1.secret")
        ))
        .unwrap();
        assert_eq!(
            basic_auth_password(Some(&header)).as_deref(),
            Some("fst1.secret")
        );
        assert_eq!(basic_auth_password(None), None);
        let bearer = http::HeaderValue::from_static("Bearer x");
        assert_eq!(basic_auth_password(Some(&bearer)), None);
    }
}
