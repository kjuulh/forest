//! Acceptance tests for the registration email-domain regex (018).
//!
//! Runs against `restricted_fixture()` which boots a second forest-server
//! instance with `FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX=@understory\.io$`
//! and `FOREST_REQUIRE_EMAIL_VERIFICATION=true`.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{Fixture, mark_email_verified, restricted_fixture};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

fn unique_username() -> String {
    format!("user-{}", uuid::Uuid::now_v7())
}

fn unique_understory_email() -> String {
    format!("kasper-{}@understory.io", uuid::Uuid::now_v7())
}

fn unique_evil_email() -> String {
    format!("attacker-{}@evil.com", uuid::Uuid::now_v7())
}

/// Register, mark email verified directly in the DB (simulating
/// what forage's verify-email flow does), then log in. Returns the
/// access token and user_id. Used by tests that need a logged-in user
/// when `require_email_verification = true`.
async fn register_verified_login(fixture: &Fixture, email: &str) -> (String, String) {
    let username = unique_username();
    let mut users = fixture.users();

    let registered = users
        .register(RegisterRequest {
            username: username.clone(),
            email: email.into(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register baseline user")
        .into_inner();

    let user_id = registered.user.expect("user").user_id;
    let user_uuid: uuid::Uuid = user_id.parse().expect("valid uuid");

    mark_email_verified(&fixture.db, user_uuid, email)
        .await
        .expect("mark email verified");

    let login_resp = users
        .login(LoginRequest {
            identifier: Some(login_request::Identifier::Email(email.into())),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("login after verifying email")
        .into_inner();

    (login_resp.tokens.expect("tokens").access_token, user_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn register_with_allowed_domain_succeeds() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let resp = users
        .register(RegisterRequest {
            username: unique_username(),
            email: unique_understory_email(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register with allowed domain")
        .into_inner();

    // With FOREST_REQUIRE_EMAIL_VERIFICATION=true, register creates the
    // user but withholds tokens until the email is verified.
    assert!(resp.tokens.is_none());
    assert!(resp.email_verification_required);
    assert!(resp.user.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn register_with_disallowed_domain_is_permission_denied() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let err = users
        .register(RegisterRequest {
            username: unique_username(),
            email: unique_evil_email(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect_err("expected disallowed domain to be rejected");

    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");

    // Error message must not leak the configured regex pattern.
    let msg = err.message();
    assert!(
        !msg.contains("understory"),
        "error message should not leak the configured pattern, got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn register_normalizes_case_before_match() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let mixed = format!("Kasper-{}@Understory.IO", uuid::Uuid::now_v7());
    users
        .register(RegisterRequest {
            username: unique_username(),
            email: mixed,
            password: "TestPassword123!".into(),
        })
        .await
        .expect("mixed-case allowed domain should be normalized and accepted");
}

#[tokio::test(flavor = "multi_thread")]
async fn add_email_with_disallowed_domain_is_blocked() {
    let fixture = restricted_fixture().await.unwrap();
    let (token, user_id) = register_verified_login(&fixture, &unique_understory_email()).await;
    let mut users = fixture.users();

    // The side-channel must close: a logged-in user cannot add a
    // disallowed-domain email.
    let err = users
        .add_email(authed_request(
            &token,
            AddEmailRequest {
                user_id,
                email: unique_evil_email(),
            },
        ))
        .await
        .expect_err("expected add_email to be rejected");

    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn add_email_with_allowed_domain_succeeds() {
    let fixture = restricted_fixture().await.unwrap();
    let (token, user_id) = register_verified_login(&fixture, &unique_understory_email()).await;
    let mut users = fixture.users();

    users
        .add_email(authed_request(
            &token,
            AddEmailRequest {
                user_id,
                email: unique_understory_email(),
            },
        ))
        .await
        .expect("add_email with allowed domain");
}
