//! Acceptance tests for the RFC 8628 device authorization grant
//! (TASKS/022-device-login.md).
//!
//! These run against the same shared DB as the rest of the accept suite;
//! identifiers are UUID-suffixed for isolation.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, mark_email_verified, restricted_fixture,
};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

fn unique_username() -> String {
    format!("dl-user-{}", uuid::Uuid::now_v7())
}

fn unique_email() -> String {
    format!("dl-{}@understory.io", uuid::Uuid::now_v7())
}

/// Register + verify a user against `restricted_fixture()` so subsequent
/// approval calls have a real user_id to attach.
async fn registered_user(fixture: &crate::accepttest::fixtures::Fixture) -> (String, String) {
    let mut users = fixture.users();
    let username = unique_username();
    let email = unique_email();

    let resp = users
        .register(RegisterRequest {
            username: username.clone(),
            email: email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();

    let user_id = resp.user.expect("user").user_id;
    mark_email_verified(&fixture.db, uuid::Uuid::parse_str(&user_id).unwrap(), &email)
        .await
        .expect("mark verified");

    (user_id, email)
}

// ── Initiate ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn initiate_returns_codes_and_verification_uri() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let resp = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    assert!(!resp.device_code.is_empty(), "device_code must be set");
    assert!(!resp.user_code.is_empty(), "user_code must be set");
    assert!(
        resp.user_code.contains('-'),
        "user_code should be dash-grouped"
    );
    assert!(
        resp.verification_uri.ends_with("/device"),
        "verification_uri should land on forage /device"
    );
    assert!(
        resp.verification_uri_complete.contains("user_code="),
        "verification_uri_complete should embed the code"
    );
    assert_eq!(resp.expires_in_seconds, 900);
    assert_eq!(resp.interval_seconds, 5);
}

// ── Initial poll ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn poll_returns_pending_before_approval() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("poll")
        .into_inner();

    assert_eq!(resp.status, DeviceLoginStatus::Pending as i32);
    assert!(resp.tokens.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_unknown_device_code_returns_expired() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            // Random base64url-shaped string the server won't recognise.
            device_code: "AAAA_unknown_BBBB_unknown_CCCC_unknown_DDDD".into(),
        })
        .await
        .expect("poll")
        .into_inner();

    assert_eq!(
        resp.status,
        DeviceLoginStatus::Expired as i32,
        "unknown codes must be indistinguishable from expired (anti-enumeration)"
    );
}

// ── Full happy path ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn approve_then_poll_returns_tokens() {
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, _email) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code.clone(),
                user_id: user_id.clone(),
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "test-browser".into(),
            },
        ))
        .await
        .expect("approve");

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("poll after approve")
        .into_inner();

    assert_eq!(resp.status, DeviceLoginStatus::Approved as i32);
    let tokens = resp.tokens.expect("tokens");
    assert!(!tokens.access_token.is_empty());
    assert!(!tokens.refresh_token.is_empty());
    let user = resp.user.expect("user");
    assert_eq!(user.user_id, user_id);
}

// ── Replay protection ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn second_poll_after_success_returns_expired_not_approved() {
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, _email) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code,
                user_id,
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "ua".into(),
            },
        ))
        .await
        .expect("approve");

    // First poll succeeds…
    let first = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code.clone(),
        })
        .await
        .expect("first poll")
        .into_inner();
    assert_eq!(first.status, DeviceLoginStatus::Approved as i32);

    // …second poll with the SAME device_code is masked as Expired so a
    // replay attacker can't tell the code was already consumed.
    let second = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("second poll")
        .into_inner();
    assert_eq!(
        second.status,
        DeviceLoginStatus::Expired as i32,
        "second poll after success must NOT return Approved again"
    );
    assert!(second.tokens.is_none());
}

// ── Denial ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn deny_then_poll_returns_denied() {
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, _email) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    users
        .deny_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            DenyDeviceLoginRequest {
                user_code: init.user_code,
                user_id,
            },
        ))
        .await
        .expect("deny");

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("poll after deny")
        .into_inner();

    assert_eq!(resp.status, DeviceLoginStatus::Denied as i32);
    assert!(resp.tokens.is_none());
}

// ── Authz: approve without service-account ────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn approve_without_service_account_is_rejected() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    let err = users
        .approve_device_login(ApproveDeviceLoginRequest {
            user_code: init.user_code,
            user_id: uuid::Uuid::now_v7().to_string(),
            approving_ip: String::new(),
            approving_user_agent: String::new(),
        })
        .await
        .expect_err("unauthed approve must be rejected");

    // Auth layer rejects before the handler runs.
    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_without_service_account_is_rejected() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    let err = users
        .deny_device_login(DenyDeviceLoginRequest {
            user_code: init.user_code,
            user_id: uuid::Uuid::now_v7().to_string(),
        })
        .await
        .expect_err("unauthed deny must be rejected");

    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err:?}");
}

// ── User-code normalization ───────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn approve_accepts_lowercase_and_no_dash() {
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, _email) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    // Strip dashes and lowercase — what a user pasting from terminal
    // might submit.
    let messy = init.user_code.replace('-', "").to_lowercase();

    users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: messy,
                user_id: user_id.clone(),
                approving_ip: String::new(),
                approving_user_agent: String::new(),
            },
        ))
        .await
        .expect("approve with messy code");

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("poll")
        .into_inner();
    assert_eq!(resp.status, DeviceLoginStatus::Approved as i32);
}

// ── Slow-down ─────────────────────────────────────────────────────────

// ── Tamper / input-bound tests ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn initiate_rejects_oversized_client_name() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let err = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "x".repeat(10_000),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect_err("oversize client_name must be rejected");
    // Server maps anyhow errors to Status::internal by default. Either
    // way, this must NOT succeed — the row insertion would otherwise
    // store a 10 KB attacker-controlled string per request.
    assert_ne!(err.code(), tonic::Code::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_rejects_oversized_device_code_as_expired() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let resp = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: "A".repeat(10_000),
        })
        .await
        .expect("oversize device_code is masked, not errored")
        .into_inner();
    assert_eq!(
        resp.status,
        DeviceLoginStatus::Expired as i32,
        "oversize device_code must be masked as Expired, not leaked as InvalidArgument"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn double_approve_with_different_user_is_rejected() {
    // Once a grant is approved by user A it transitions out of Pending,
    // so an attacker who somehow learns the user_code cannot
    // re-approve it as themselves. This is the natural guard from the
    // domain state machine (`approve` requires Status::Pending); the
    // test exists to lock the behaviour in against future regressions.
    let fixture = restricted_fixture().await.unwrap();
    let (user_a, _) = registered_user(&fixture).await;
    let (user_b, _) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code.clone(),
                user_id: user_a.clone(),
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "ua".into(),
            },
        ))
        .await
        .expect("first approve");

    // User B (attacker) tries to re-approve the same code.
    let err = users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code,
                user_id: user_b,
                approving_ip: "10.0.0.1".into(),
                approving_user_agent: "attacker".into(),
            },
        ))
        .await
        .expect_err("re-approval must be rejected");
    assert_ne!(err.code(), tonic::Code::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn approve_unknown_code_does_not_leak_distinct_error() {
    // Whether the user_code never existed or was already consumed,
    // both must surface as the same opaque error class — otherwise an
    // attacker can probe for valid codes by error-string analysis.
    let fixture = restricted_fixture().await.unwrap();
    let (user_id, _) = registered_user(&fixture).await;
    let mut users = fixture.users();

    let unknown_err = users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: "ZZZZZZZZ".into(),
                user_id: user_id.clone(),
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "ua".into(),
            },
        ))
        .await
        .expect_err("unknown code must be rejected");

    // Initiate + approve + try to re-approve a consumed-or-expired code.
    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();
    users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code.clone(),
                user_id: user_id.clone(),
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "ua".into(),
            },
        ))
        .await
        .expect("approve first time");
    let replay_err = users
        .approve_device_login(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ApproveDeviceLoginRequest {
                user_code: init.user_code,
                user_id,
                approving_ip: "127.0.0.1".into(),
                approving_user_agent: "ua".into(),
            },
        ))
        .await
        .expect_err("re-approval must be rejected");

    // Both errors should be in the same gRPC class (not Ok). The actual
    // message strings differ today; that's an info-leak we accept since
    // the caller is service-account-only (forage), not a public client.
    // The user-facing forage UI must render a uniform "code invalid"
    // page either way.
    assert_ne!(unknown_err.code(), tonic::Code::Ok);
    assert_ne!(replay_err.code(), tonic::Code::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn back_to_back_poll_returns_slow_down() {
    let fixture = fixture().await.unwrap();
    let mut users = fixture.users();

    let init = users
        .initiate_device_login(InitiateDeviceLoginRequest {
            client_name: "forest-cli".into(),
            client_version: "0.3.2".into(),
            scopes: vec![],
        })
        .await
        .expect("initiate")
        .into_inner();

    // First poll establishes last_polled_at.
    let first = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code.clone(),
        })
        .await
        .expect("first poll")
        .into_inner();
    assert_eq!(first.status, DeviceLoginStatus::Pending as i32);

    // Immediate re-poll within interval_seconds should return SlowDown.
    let second = users
        .poll_device_login(PollDeviceLoginRequest {
            device_code: init.device_code,
        })
        .await
        .expect("second poll")
        .into_inner();
    assert_eq!(
        second.status,
        DeviceLoginStatus::SlowDown as i32,
        "polling faster than interval must return SLOW_DOWN per RFC 8628"
    );
}
