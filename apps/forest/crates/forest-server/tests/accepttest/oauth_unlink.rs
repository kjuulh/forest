//! Acceptance tests for `UnlinkOAuthProvider` — DATA-247.
//!
//! The unlink RPC must refuse to strip the last sign-in method from an
//! account. Three scenarios:
//!
//! 1. Password user unlinks a linked OAuth provider: succeeds (password
//!    remains as a sign-in method).
//! 2. OAuth-only user with two linked providers unlinks one: succeeds
//!    (the other provider remains).
//! 3. OAuth-only user with one linked provider unlinks it: fails with
//!    `FailedPrecondition("last_auth_method")` — that's the only way
//!    they can sign in.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{Fixture, fixture};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

fn unique_username() -> String {
    format!("unlink-user-{}", uuid::Uuid::now_v7())
}

fn unique_email() -> String {
    format!("unlink-{}@example.com", uuid::Uuid::now_v7())
}

/// Register a user (gets a password and a `native` identity row), return
/// the access token and user_id.
async fn register(fixture: &Fixture) -> (String, String) {
    let mut users = fixture.users();
    let resp = users
        .register(RegisterRequest {
            username: unique_username(),
            email: unique_email(),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let user_id = resp.user.expect("user").user_id;
    let tokens = resp.tokens.expect("tokens");
    (tokens.access_token, user_id)
}

/// Strip the password and the `native` identity row so the user behaves
/// like an OAuth-only account. Acceptance tests use this rather than
/// going through the OAuth login RPC because the latter requires a
/// service-account token and a fully-stubbed provider exchange.
async fn make_oauth_only(db: &sqlx::PgPool, user_id: &str) {
    let uid: uuid::Uuid = user_id.parse().expect("valid uuid");
    sqlx::query!(
        "DELETE FROM provider_native_credentials WHERE user_id = $1",
        uid,
    )
    .execute(db)
    .await
    .expect("delete native credential");
    sqlx::query!(
        "DELETE FROM identities WHERE user_id = $1 AND provider = 'native'",
        uid,
    )
    .execute(db)
    .await
    .expect("delete native identity");
}

async fn link_provider(
    fixture: &Fixture,
    token: &str,
    user_id: &str,
    provider: OAuthProvider,
    external_id: &str,
) {
    let mut users = fixture.users();
    users
        .link_o_auth_provider(authed_request(
            token,
            LinkOAuthProviderRequest {
                user_id: user_id.into(),
                provider: provider as i32,
                provider_user_id: external_id.into(),
                provider_email: format!("{external_id}@example.com"),
                provider_display_name: external_id.into(),
                provider_data_json: String::new(),
            },
        ))
        .await
        .unwrap_or_else(|e| panic!("link {provider:?}: {e:?}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_succeeds_when_password_remains() {
    let fixture = fixture().await.unwrap();
    let (token, user_id) = register(&fixture).await;

    link_provider(
        &fixture,
        &token,
        &user_id,
        OAuthProvider::OauthProviderGithub,
        &format!("gh-{}", uuid::Uuid::now_v7()),
    )
    .await;

    let mut users = fixture.users();
    users
        .unlink_o_auth_provider(authed_request(
            &token,
            UnlinkOAuthProviderRequest {
                user_id: user_id.clone(),
                provider: OAuthProvider::OauthProviderGithub as i32,
            },
        ))
        .await
        .expect("unlink should succeed when password remains");
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_succeeds_when_another_provider_remains() {
    let fixture = fixture().await.unwrap();
    let (token, user_id) = register(&fixture).await;

    link_provider(
        &fixture,
        &token,
        &user_id,
        OAuthProvider::OauthProviderGithub,
        &format!("gh-{}", uuid::Uuid::now_v7()),
    )
    .await;
    link_provider(
        &fixture,
        &token,
        &user_id,
        OAuthProvider::OauthProviderGoogle,
        &format!("g-{}", uuid::Uuid::now_v7()),
    )
    .await;
    make_oauth_only(&fixture.db, &user_id).await;

    let mut users = fixture.users();
    users
        .unlink_o_auth_provider(authed_request(
            &token,
            UnlinkOAuthProviderRequest {
                user_id: user_id.clone(),
                provider: OAuthProvider::OauthProviderGithub as i32,
            },
        ))
        .await
        .expect("unlink should succeed when another provider remains");
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_fails_when_it_would_be_last_auth_method() {
    let fixture = fixture().await.unwrap();
    let (token, user_id) = register(&fixture).await;

    link_provider(
        &fixture,
        &token,
        &user_id,
        OAuthProvider::OauthProviderGithub,
        &format!("gh-{}", uuid::Uuid::now_v7()),
    )
    .await;
    make_oauth_only(&fixture.db, &user_id).await;

    let mut users = fixture.users();
    let err = users
        .unlink_o_auth_provider(authed_request(
            &token,
            UnlinkOAuthProviderRequest {
                user_id: user_id.clone(),
                provider: OAuthProvider::OauthProviderGithub as i32,
            },
        ))
        .await
        .expect_err("unlink should be blocked");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition, "{err:?}");
    assert_eq!(
        err.message(),
        "last_auth_method",
        "wire code must be stable for callers"
    );

    // The github identity must still be present after a refused unlink.
    let still_linked = sqlx::query_scalar!(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM identities
            WHERE user_id = $1 AND provider = 'oauth_provider_github'
        ) AS "exists!"
        "#,
        user_id.parse::<uuid::Uuid>().unwrap(),
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert!(still_linked, "identity should not have been deleted");
}
