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
