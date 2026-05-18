//! Acceptance tests for the email-verification flow at signup (019).
//!
//! Runs against `restricted_fixture()` which has both
//! `FOREST_REGISTRATION_EMAIL_DOMAIN_REGEX=@understory\.io$` and
//! `FOREST_REQUIRE_EMAIL_VERIFICATION=true`.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, restricted_fixture,
};

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

#[tokio::test(flavor = "multi_thread")]
async fn register_returns_no_tokens_when_verification_required() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let resp = users
        .register(RegisterRequest {
            username: unique_username(),
            email: unique_understory_email(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();

    assert!(
        resp.tokens.is_none(),
        "register should not issue tokens when verification is required"
    );
    assert!(resp.email_verification_required);
    assert!(resp.user.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn login_with_unverified_email_returns_failed_precondition() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let email = unique_understory_email();
    let username = unique_username();

    users
        .register(RegisterRequest {
            username: username.clone(),
            email: email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register");

    let err = users
        .login(LoginRequest {
            identifier: Some(login_request::Identifier::Email(email)),
            password: "TestPassword123!".into(),
        })
        .await
        .expect_err("expected login to be blocked on unverified email");

    assert_eq!(
        err.code(),
        tonic::Code::FailedPrecondition,
        "expected FailedPrecondition, got {err:?}"
    );
    // Forage parses this canonical detail string; do not change it
    // without coordinating the forage handler.
    assert_eq!(err.message(), "email_not_verified", "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn confirm_email_verification_marks_email_verified() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let email = unique_understory_email();

    let registered = users
        .register(RegisterRequest {
            username: unique_username(),
            email: email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let user_id = registered.user.expect("user").user_id;

    // Service-account caller (forage stand-in) confirms verification.
    users
        .confirm_email_verification(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ConfirmEmailVerificationRequest {
                email: email.clone(),
            },
        ))
        .await
        .expect("confirm email verification");

    // Login should now succeed.
    let login = users
        .login(LoginRequest {
            identifier: Some(login_request::Identifier::Email(email)),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("login after verification")
        .into_inner();

    assert!(login.tokens.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn confirm_email_verification_without_service_account_is_rejected() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let email = unique_understory_email();

    let registered = users
        .register(RegisterRequest {
            username: unique_username(),
            email: email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let _ = registered.user.expect("user").user_id;

    // Unauthenticated.
    let err = users
        .confirm_email_verification(ConfirmEmailVerificationRequest {
            email: email.clone(),
        })
        .await
        .expect_err("unauthed call must be rejected");
    // Auth layer denies before the handler runs.
    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_email_with_user_jwt_for_other_user_is_rejected() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    // Register attacker, confirm verification, log in.
    let attacker_email = unique_understory_email();
    let attacker_reg = users
        .register(RegisterRequest {
            username: unique_username(),
            email: attacker_email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register attacker")
        .into_inner();
    let _ = attacker_reg.user.expect("user").user_id;

    users
        .confirm_email_verification(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ConfirmEmailVerificationRequest {
                email: attacker_email.clone(),
            },
        ))
        .await
        .expect("confirm attacker email");

    let attacker_login = users
        .login(LoginRequest {
            identifier: Some(login_request::Identifier::Email(attacker_email)),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("login attacker")
        .into_inner();
    let attacker_token = attacker_login.tokens.expect("tokens").access_token;

    // Register victim (still unverified).
    let victim_email = unique_understory_email();
    let victim_reg = users
        .register(RegisterRequest {
            username: unique_username(),
            email: victim_email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register victim")
        .into_inner();
    let victim_user_id = victim_reg.user.expect("user").user_id;

    // Attacker tries to verify the victim's email using attacker's JWT.
    let err = users
        .verify_email(authed_request(
            &attacker_token,
            // Attacker uses their JWT but supplies the victim's user_id;
            // forest's verify_email must reject this because the JWT
            // claim doesn't match req.user_id.
            VerifyEmailRequest {
                user_id: victim_user_id,
                email: victim_email,
            },
        ))
        .await
        .expect_err("cross-user verify_email must be rejected");

    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn add_email_response_signals_verification_required() {
    let fixture = restricted_fixture().await.unwrap();
    let mut users = fixture.users();

    let email = unique_understory_email();
    let registered = users
        .register(RegisterRequest {
            username: unique_username(),
            email: email.clone(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let user_id = registered.user.expect("user").user_id;

    // Verify the primary email + log in.
    users
        .confirm_email_verification(authed_request(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            ConfirmEmailVerificationRequest {
                email: email.clone(),
            },
        ))
        .await
        .expect("confirm primary email");
    let login = users
        .login(LoginRequest {
            identifier: Some(login_request::Identifier::Email(email)),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("login")
        .into_inner();
    let token = login.tokens.expect("tokens").access_token;

    let secondary = unique_understory_email();
    let resp = users
        .add_email(authed_request(
            &token,
            AddEmailRequest {
                user_id,
                email: secondary,
            },
        ))
        .await
        .expect("add_email")
        .into_inner();

    assert!(
        resp.email_verification_required,
        "add_email must signal verification when require_email_verification=true"
    );
    let added = resp.email.expect("email");
    assert!(!added.verified, "newly added email must start unverified");
}
