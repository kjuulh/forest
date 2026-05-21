//! Device-login grant aggregate (RFC 8628 device authorization grant).
//!
//! Pure domain core for the `forest auth login --web` flow. See
//! `apps/forest/TASKS/022-device-login.md` for the full spec.
//!
//! The state machine is:
//!
//! ```text
//!     NonExistent
//!         │ initiate
//!         ▼
//!      Pending ──── approve ────► Approved ──── consume ────► Consumed
//!         │                          │                          │
//!         │ deny                     │ (TTL elapses             │ (terminal,
//!         ▼                          │  before consume)         │  externally
//!       Denied                       ▼                          │  masked as
//!         │ (TTL elapses)         Expired                       │  Expired)
//!         ▼
//!      Expired
//! ```
//!
//! `Consumed` is an internal terminal state distinguished from `Expired`
//! so a replay attacker cannot mint a second token pair from the same
//! device_code. Externally (via the gRPC `Poll` response) it is reported
//! as `Expired` — indistinguishable from a normal TTL expiry.

use anyhow::bail;
use chrono::{DateTime, Utc};
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ============================================================
// Events
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DeviceGrantEvent {
    Initiated {
        grant_id: Uuid,
        device_code_hash: String,
        user_code: String,
        client_name: String,
        client_version: String,
        scopes: Vec<String>,
        expires_at: DateTime<Utc>,
        interval_seconds: i32,
    },
    Approved {
        user_id: Uuid,
        approving_ip: String,
        approving_user_agent: String,
        at: DateTime<Utc>,
    },
    Denied {
        user_id: Uuid,
        at: DateTime<Utc>,
    },
    Consumed {
        at: DateTime<Utc>,
    },
    Expired {
        at: DateTime<Utc>,
    },
}

impl EventData for DeviceGrantEvent {
    fn event_type(&self) -> &'static str {
        match self {
            DeviceGrantEvent::Initiated { .. } => "device_grant.initiated",
            DeviceGrantEvent::Approved { .. } => "device_grant.approved",
            DeviceGrantEvent::Denied { .. } => "device_grant.denied",
            DeviceGrantEvent::Consumed { .. } => "device_grant.consumed",
            DeviceGrantEvent::Expired { .. } => "device_grant.expired",
        }
    }
}

// ============================================================
// Aggregate state
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceGrantStatus {
    NonExistent,
    Pending,
    Approved,
    Consumed,
    Denied,
    Expired,
}

#[derive(Debug)]
pub struct DeviceGrantAggregate {
    pub status: DeviceGrantStatus,
    pub grant_id: Option<Uuid>,
    pub device_code_hash: String,
    pub user_code: String,
    pub client_name: String,
    pub client_version: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub interval_seconds: i32,
    pub approved_user_id: Option<Uuid>,
    pub approving_ip: Option<String>,
    pub approving_user_agent: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    pub consumed_at: Option<DateTime<Utc>>,
}

impl Default for DeviceGrantAggregate {
    fn default() -> Self {
        Self {
            status: DeviceGrantStatus::NonExistent,
            grant_id: None,
            device_code_hash: String::new(),
            user_code: String::new(),
            client_name: String::new(),
            client_version: String::new(),
            scopes: Vec::new(),
            expires_at: None,
            interval_seconds: 0,
            approved_user_id: None,
            approving_ip: None,
            approving_user_agent: None,
            approved_at: None,
            consumed_at: None,
        }
    }
}

impl Aggregate for DeviceGrantAggregate {
    type Event = DeviceGrantEvent;

    fn stream_category() -> StreamCategory {
        "device_grant".into_stream_category()
    }

    fn apply(&mut self, event: &DeviceGrantEvent) {
        match event {
            DeviceGrantEvent::Initiated {
                grant_id,
                device_code_hash,
                user_code,
                client_name,
                client_version,
                scopes,
                expires_at,
                interval_seconds,
            } => {
                self.status = DeviceGrantStatus::Pending;
                self.grant_id = Some(*grant_id);
                self.device_code_hash.clone_from(device_code_hash);
                self.user_code.clone_from(user_code);
                self.client_name.clone_from(client_name);
                self.client_version.clone_from(client_version);
                self.scopes.clone_from(scopes);
                self.expires_at = Some(*expires_at);
                self.interval_seconds = *interval_seconds;
            }
            DeviceGrantEvent::Approved {
                user_id,
                approving_ip,
                approving_user_agent,
                at,
            } => {
                self.status = DeviceGrantStatus::Approved;
                self.approved_user_id = Some(*user_id);
                self.approving_ip = Some(approving_ip.clone());
                self.approving_user_agent = Some(approving_user_agent.clone());
                self.approved_at = Some(*at);
            }
            DeviceGrantEvent::Denied { user_id, .. } => {
                self.status = DeviceGrantStatus::Denied;
                // Record who denied — useful for audit even though tokens never issue.
                self.approved_user_id = Some(*user_id);
            }
            DeviceGrantEvent::Consumed { at } => {
                self.status = DeviceGrantStatus::Consumed;
                self.consumed_at = Some(*at);
            }
            DeviceGrantEvent::Expired { .. } => {
                self.status = DeviceGrantStatus::Expired;
            }
        }
    }
}

// ============================================================
// External poll-status mapping
// ============================================================

/// Status returned to the polling CLI. Distinct from the internal
/// [`DeviceGrantStatus`] so that `Consumed` is masked as `Expired`
/// (preventing a replay attacker from distinguishing a stolen-but-used
/// device_code from a naturally-expired one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollStatus {
    Pending,
    /// The grant is approved and not yet consumed. The caller should now
    /// call [`DeviceGrantAggregate::consume`] and return tokens to the CLI.
    Approved,
    Denied,
    Expired,
}

impl DeviceGrantAggregate {
    /// Compute what the CLI poller should see right now.
    ///
    /// Idempotent and read-only — emitting an `Expired` event for grants
    /// past their TTL is the responsibility of the background sweep,
    /// not the poller.
    pub fn poll_status(&self, now: DateTime<Utc>) -> PollStatus {
        let past_ttl = self
            .expires_at
            .map(|exp| now >= exp)
            .unwrap_or(false);

        match self.status {
            DeviceGrantStatus::NonExistent => PollStatus::Expired,
            DeviceGrantStatus::Pending if past_ttl => PollStatus::Expired,
            DeviceGrantStatus::Pending => PollStatus::Pending,
            DeviceGrantStatus::Approved if past_ttl => PollStatus::Expired,
            DeviceGrantStatus::Approved => PollStatus::Approved,
            DeviceGrantStatus::Consumed => PollStatus::Expired,
            DeviceGrantStatus::Denied => PollStatus::Denied,
            DeviceGrantStatus::Expired => PollStatus::Expired,
        }
    }
}

// ============================================================
// Commands (pure business logic)
// ============================================================

pub struct InitiateDeviceGrantParams {
    pub device_code_hash: String,
    pub user_code: String,
    pub client_name: String,
    pub client_version: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub interval_seconds: i32,
}

impl DeviceGrantAggregate {
    pub fn initiate(
        root: &mut AggregateRoot<Self>,
        params: InitiateDeviceGrantParams,
    ) -> anyhow::Result<Uuid> {
        match root.state.status {
            DeviceGrantStatus::NonExistent => {}
            _ => bail!("device grant already initiated"),
        }
        if params.device_code_hash.is_empty() {
            bail!("device_code_hash must not be empty");
        }
        if params.user_code.is_empty() {
            bail!("user_code must not be empty");
        }
        if params.interval_seconds < 1 {
            bail!("interval_seconds must be >= 1");
        }

        let grant_id = Uuid::now_v7();

        root.record(DeviceGrantEvent::Initiated {
            grant_id,
            device_code_hash: params.device_code_hash,
            user_code: params.user_code,
            client_name: params.client_name,
            client_version: params.client_version,
            scopes: params.scopes,
            expires_at: params.expires_at,
            interval_seconds: params.interval_seconds,
        });

        Ok(grant_id)
    }

    pub fn approve(
        root: &mut AggregateRoot<Self>,
        user_id: Uuid,
        approving_ip: String,
        approving_user_agent: String,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match root.state.status {
            DeviceGrantStatus::Pending => {}
            DeviceGrantStatus::NonExistent => bail!("device grant does not exist"),
            DeviceGrantStatus::Approved => bail!("device grant already approved"),
            DeviceGrantStatus::Consumed => bail!("device grant already consumed"),
            DeviceGrantStatus::Denied => bail!("device grant was denied"),
            DeviceGrantStatus::Expired => bail!("device grant has expired"),
        }
        if let Some(exp) = root.state.expires_at {
            if now >= exp {
                bail!("device grant has expired");
            }
        }

        root.record(DeviceGrantEvent::Approved {
            user_id,
            approving_ip,
            approving_user_agent,
            at: now,
        });
        Ok(())
    }

    pub fn deny(
        root: &mut AggregateRoot<Self>,
        user_id: Uuid,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match root.state.status {
            DeviceGrantStatus::Pending => {}
            DeviceGrantStatus::NonExistent => bail!("device grant does not exist"),
            DeviceGrantStatus::Approved => bail!("device grant already approved"),
            DeviceGrantStatus::Consumed => bail!("device grant already consumed"),
            DeviceGrantStatus::Denied => bail!("device grant already denied"),
            DeviceGrantStatus::Expired => bail!("device grant has expired"),
        }
        if let Some(exp) = root.state.expires_at {
            if now >= exp {
                bail!("device grant has expired");
            }
        }

        root.record(DeviceGrantEvent::Denied { user_id, at: now });
        Ok(())
    }

    /// Mark an approved grant as consumed. Called by the service layer
    /// when a successful poll returns tokens to the CLI. Single-use:
    /// a second call returns Err.
    pub fn consume(
        root: &mut AggregateRoot<Self>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match root.state.status {
            DeviceGrantStatus::Approved => {}
            DeviceGrantStatus::NonExistent => bail!("device grant does not exist"),
            DeviceGrantStatus::Pending => bail!("device grant not yet approved"),
            DeviceGrantStatus::Consumed => bail!("device grant already consumed"),
            DeviceGrantStatus::Denied => bail!("device grant was denied"),
            DeviceGrantStatus::Expired => bail!("device grant has expired"),
        }
        if let Some(exp) = root.state.expires_at {
            if now >= exp {
                bail!("device grant has expired");
            }
        }

        root.record(DeviceGrantEvent::Consumed { at: now });
        Ok(())
    }

    /// Mark a non-terminal grant as expired. Idempotent on already-terminal
    /// states (returns Ok with no event recorded). Intended for a periodic
    /// sweep job that walks grants whose `expires_at` has passed.
    pub fn expire(
        root: &mut AggregateRoot<Self>,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match root.state.status {
            DeviceGrantStatus::NonExistent => bail!("device grant does not exist"),
            DeviceGrantStatus::Pending | DeviceGrantStatus::Approved => {}
            DeviceGrantStatus::Consumed
            | DeviceGrantStatus::Denied
            | DeviceGrantStatus::Expired => return Ok(()),
        }

        root.record(DeviceGrantEvent::Expired { at: now });
        Ok(())
    }
}

/// Stream key for a device-grant aggregate: the `grant_id` UUID as string.
/// Forage looks up grants by `user_code`; the service layer translates that
/// to a `grant_id` via the projection table.
pub fn stream_key(grant_id: &Uuid) -> String {
    grant_id.to_string()
}

// ============================================================
// Code generators (RFC 8628 §6.1 — user_code and device_code)
// ============================================================

/// Unambiguous user-code alphabet. 32 symbols, no vowels (avoids accidental
/// words), no 0/O/1/I/L (avoids visual confusion). 32 symbols gives exactly
/// 5 bits per character → 40 bits of entropy for an 8-char code.
pub const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789";

/// Length of a generated user code (before grouping with a dash).
pub const USER_CODE_LEN: usize = 8;

/// Generate a user code as `XXXX-XXXX` from the unambiguous alphabet.
pub fn generate_user_code<R: Rng + ?Sized>(rng: &mut R) -> String {
    debug_assert_eq!(USER_CODE_ALPHABET.len(), 28); // panics in tests if alphabet edited inconsistently
    let alphabet_len = USER_CODE_ALPHABET.len() as u32;
    let mut chars = Vec::with_capacity(USER_CODE_LEN + 1);
    for i in 0..USER_CODE_LEN {
        let idx = (rng.next_u32() % alphabet_len) as usize;
        chars.push(USER_CODE_ALPHABET[idx]);
        if i == 3 {
            chars.push(b'-');
        }
    }
    String::from_utf8(chars).expect("alphabet is ASCII")
}

/// Normalize a user-supplied code: uppercase, strip dashes and whitespace.
/// Forage's approval endpoint calls this before lookup so the user can
/// paste `abcdefgh`, `abcd efgh`, or `ABCD-EFGH` interchangeably.
pub fn normalize_user_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

/// Length in bytes of the random material for a device_code (≥ 128 bits per
/// RFC 8628 §5.2; we use 256 to match other forest secrets).
pub const DEVICE_CODE_BYTES: usize = 32;

/// Generate a device_code: 32 random bytes, base64url-no-pad encoded.
pub fn generate_device_code<R: Rng + ?Sized>(rng: &mut R) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;

    let mut buf = [0u8; DEVICE_CODE_BYTES];
    rng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// SHA-256 hash of a device_code, hex-encoded. The hash — not the raw code —
/// is what we store in the projection and use as the stream lookup key.
pub fn hash_device_code(device_code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(device_code.as_bytes());
    hex::encode(hasher.finalize())
}

// ============================================================
// Unit tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rand::{rngs::StdRng, SeedableRng};

    fn fresh_root() -> AggregateRoot<DeviceGrantAggregate> {
        AggregateRoot::new("device_grant-0190abcd".into())
    }

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn default_params() -> InitiateDeviceGrantParams {
        InitiateDeviceGrantParams {
            device_code_hash: hash_device_code("test-device-code"),
            user_code: "ABCDEFGH".into(),
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
            expires_at: t0() + Duration::seconds(900),
            interval_seconds: 5,
        }
    }

    // ---- initiate ----

    #[test]
    fn initiate_records_initiated_event() {
        let mut root = fresh_root();
        let id = DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        assert_eq!(root.state.status, DeviceGrantStatus::Pending);
        assert_eq!(root.state.grant_id, Some(id));
        assert_eq!(root.state.user_code, "ABCDEFGH");
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn initiate_rejects_when_already_initiated() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        assert!(DeviceGrantAggregate::initiate(&mut root, default_params()).is_err());
    }

    #[test]
    fn initiate_rejects_empty_user_code() {
        let mut root = fresh_root();
        let mut p = default_params();
        p.user_code = String::new();
        assert!(DeviceGrantAggregate::initiate(&mut root, p).is_err());
    }

    #[test]
    fn initiate_rejects_empty_device_code_hash() {
        let mut root = fresh_root();
        let mut p = default_params();
        p.device_code_hash = String::new();
        assert!(DeviceGrantAggregate::initiate(&mut root, p).is_err());
    }

    #[test]
    fn initiate_rejects_non_positive_interval() {
        let mut root = fresh_root();
        let mut p = default_params();
        p.interval_seconds = 0;
        assert!(DeviceGrantAggregate::initiate(&mut root, p).is_err());
    }

    // ---- approve ----

    #[test]
    fn approve_from_pending_succeeds() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        let user = Uuid::now_v7();
        DeviceGrantAggregate::approve(
            &mut root,
            user,
            "1.2.3.4".into(),
            "browser".into(),
            t0(),
        )
        .unwrap();
        assert_eq!(root.state.status, DeviceGrantStatus::Approved);
        assert_eq!(root.state.approved_user_id, Some(user));
        assert_eq!(root.state.approving_ip.as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn approve_rejects_when_nonexistent() {
        let mut root = fresh_root();
        assert!(DeviceGrantAggregate::approve(
            &mut root,
            Uuid::now_v7(),
            "ip".into(),
            "ua".into(),
            t0()
        )
        .is_err());
    }

    #[test]
    fn approve_rejects_after_ttl() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        let later = t0() + Duration::seconds(901); // past expires_at
        assert!(DeviceGrantAggregate::approve(
            &mut root,
            Uuid::now_v7(),
            "ip".into(),
            "ua".into(),
            later,
        )
        .is_err());
        assert_eq!(root.state.status, DeviceGrantStatus::Pending); // unchanged
    }

    #[test]
    fn approve_rejects_double_approve() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        assert!(DeviceGrantAggregate::approve(
            &mut root,
            Uuid::now_v7(),
            "ip".into(),
            "ua".into(),
            t0()
        )
        .is_err());
    }

    // ---- deny ----

    #[test]
    fn deny_from_pending_succeeds() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::deny(&mut root, Uuid::now_v7(), t0()).unwrap();
        assert_eq!(root.state.status, DeviceGrantStatus::Denied);
    }

    #[test]
    fn deny_rejects_after_approve() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        assert!(DeviceGrantAggregate::deny(&mut root, Uuid::now_v7(), t0()).is_err());
    }

    // ---- consume ----

    #[test]
    fn consume_from_approved_succeeds() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        DeviceGrantAggregate::consume(&mut root, t0()).unwrap();
        assert_eq!(root.state.status, DeviceGrantStatus::Consumed);
    }

    #[test]
    fn consume_rejects_when_pending() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        assert!(DeviceGrantAggregate::consume(&mut root, t0()).is_err());
    }

    #[test]
    fn consume_rejects_double_consume() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        DeviceGrantAggregate::consume(&mut root, t0()).unwrap();
        assert!(DeviceGrantAggregate::consume(&mut root, t0()).is_err());
    }

    #[test]
    fn consume_rejects_after_ttl_even_if_approved() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        let later = t0() + Duration::seconds(901);
        assert!(DeviceGrantAggregate::consume(&mut root, later).is_err());
    }

    // ---- expire ----

    #[test]
    fn expire_from_pending_transitions() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::expire(&mut root, t0() + Duration::seconds(901)).unwrap();
        assert_eq!(root.state.status, DeviceGrantStatus::Expired);
    }

    #[test]
    fn expire_idempotent_on_terminal() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::deny(&mut root, Uuid::now_v7(), t0()).unwrap();
        let before = root.pending_count();
        DeviceGrantAggregate::expire(&mut root, t0() + Duration::seconds(901)).unwrap();
        assert_eq!(root.pending_count(), before); // no new event
        assert_eq!(root.state.status, DeviceGrantStatus::Denied);
    }

    // ---- poll_status ----

    #[test]
    fn poll_status_pending_when_fresh() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        assert_eq!(root.state.poll_status(t0()), PollStatus::Pending);
    }

    #[test]
    fn poll_status_expired_when_pending_past_ttl() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        let later = t0() + Duration::seconds(901);
        assert_eq!(root.state.poll_status(later), PollStatus::Expired);
    }

    #[test]
    fn poll_status_approved_when_fresh() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        assert_eq!(root.state.poll_status(t0()), PollStatus::Approved);
    }

    #[test]
    fn poll_status_expired_when_approved_past_ttl() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        let later = t0() + Duration::seconds(901);
        assert_eq!(root.state.poll_status(later), PollStatus::Expired);
    }

    #[test]
    fn poll_status_consumed_masks_as_expired() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        DeviceGrantAggregate::consume(&mut root, t0()).unwrap();
        // Even still within TTL, a consumed grant is reported as Expired
        // to the poller — prevents replay attackers from distinguishing
        // a stolen-and-used device_code from natural expiry.
        assert_eq!(root.state.poll_status(t0()), PollStatus::Expired);
    }

    #[test]
    fn poll_status_denied_reports_denied() {
        let mut root = fresh_root();
        DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::deny(&mut root, Uuid::now_v7(), t0()).unwrap();
        assert_eq!(root.state.poll_status(t0()), PollStatus::Denied);
    }

    #[test]
    fn poll_status_nonexistent_reports_expired() {
        let root = fresh_root();
        // Never initiated. Treat lookup miss as Expired to avoid leaking
        // existence to brute-forcers.
        assert_eq!(root.state.poll_status(t0()), PollStatus::Expired);
    }

    // ---- hydration ----

    #[test]
    fn hydrate_full_lifecycle() {
        let mut root = fresh_root();
        let id = DeviceGrantAggregate::initiate(&mut root, default_params()).unwrap();
        DeviceGrantAggregate::approve(&mut root, Uuid::now_v7(), "ip".into(), "ua".into(), t0())
            .unwrap();
        DeviceGrantAggregate::consume(&mut root, t0()).unwrap();

        let events: Vec<_> = root
            .take_pending()
            .into_iter()
            .enumerate()
            .map(|(i, e)| forest_event_store::RecordedEvent {
                global_position: i as i64 + 1,
                stream_id: "device_grant-0190abcd".into(),
                stream_version: i as i64 + 1,
                event_type: e.event_type().into(),
                data: serde_json::to_value(&e).unwrap(),
                metadata: serde_json::json!({}),
                created_at: Utc::now(),
            })
            .collect();

        assert_eq!(events.len(), 3);

        let replayed = AggregateRoot::<DeviceGrantAggregate>::hydrate(
            "device_grant-0190abcd".into(),
            &events,
            events.len() as i64,
        );

        assert_eq!(replayed.state.status, DeviceGrantStatus::Consumed);
        assert_eq!(replayed.state.grant_id, Some(id));
    }

    // ---- serde ----

    #[test]
    fn event_serde_roundtrip() {
        let events = vec![
            DeviceGrantEvent::Initiated {
                grant_id: Uuid::now_v7(),
                device_code_hash: "abc".into(),
                user_code: "ABCDEFGH".into(),
                client_name: "forest-cli".into(),
                client_version: "0.3.2".into(),
                scopes: vec!["a".into()],
                expires_at: t0(),
                interval_seconds: 5,
            },
            DeviceGrantEvent::Approved {
                user_id: Uuid::now_v7(),
                approving_ip: "1.2.3.4".into(),
                approving_user_agent: "ua".into(),
                at: t0(),
            },
            DeviceGrantEvent::Denied {
                user_id: Uuid::now_v7(),
                at: t0(),
            },
            DeviceGrantEvent::Consumed { at: t0() },
            DeviceGrantEvent::Expired { at: t0() },
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let back: DeviceGrantEvent = serde_json::from_value(json).unwrap();
            assert_eq!(event.event_type(), back.event_type());
        }
    }

    // ---- code generators ----

    #[test]
    fn user_code_only_uses_alphabet() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..1_000 {
            let code = generate_user_code(&mut rng);
            assert_eq!(code.len(), USER_CODE_LEN + 1, "code = {code:?}");
            assert_eq!(code.chars().nth(4), Some('-'), "code = {code:?}");
            for (i, c) in code.chars().enumerate() {
                if i == 4 {
                    continue;
                }
                assert!(
                    USER_CODE_ALPHABET.contains(&(c as u8)),
                    "char {c:?} (at {i}) not in alphabet — code = {code:?}"
                );
            }
        }
    }

    #[test]
    fn user_code_has_reasonable_distribution() {
        // Crude smoke test: across 5000 codes, every alphabet symbol should
        // appear at least once. With 32 symbols and 8 char/code that's
        // ~10000 char draws; missing a symbol entirely would suggest a
        // generator bug.
        let mut rng = StdRng::seed_from_u64(7);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5_000 {
            for c in generate_user_code(&mut rng).chars().filter(|c| *c != '-') {
                seen.insert(c as u8);
            }
        }
        for &b in USER_CODE_ALPHABET {
            assert!(seen.contains(&b), "symbol {:?} never generated", b as char);
        }
    }

    #[test]
    fn normalize_user_code_is_case_and_dash_insensitive() {
        assert_eq!(normalize_user_code("ABCD-EFGH"), "ABCDEFGH");
        assert_eq!(normalize_user_code("abcd-efgh"), "ABCDEFGH");
        assert_eq!(normalize_user_code("  abcd efgh  "), "ABCDEFGH");
        assert_eq!(normalize_user_code("ABCDEFGH"), "ABCDEFGH");
    }

    #[test]
    fn device_code_is_long_and_url_safe() {
        let mut rng = StdRng::seed_from_u64(123);
        let code = generate_device_code(&mut rng);
        // 32 bytes base64url-no-pad → 43 chars
        assert_eq!(code.len(), 43);
        for c in code.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-url-safe char in device_code: {c:?}"
            );
        }
    }

    #[test]
    fn device_code_is_unique_across_draws() {
        let mut rng = StdRng::seed_from_u64(99);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(generate_device_code(&mut rng)));
        }
    }

    #[test]
    fn hash_device_code_is_deterministic_and_hex_64() {
        let h1 = hash_device_code("foo");
        let h2 = hash_device_code("foo");
        let h3 = hash_device_code("bar");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
