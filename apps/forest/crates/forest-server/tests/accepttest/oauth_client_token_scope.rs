//! OAuth client-credentials tokens against the gRPC API: what a machine token
//! may and may not do.
//!
//! Such a token authenticates as its app, and an app passes org-member checks
//! in its own organisation, so these pin the boundary from both sides: the
//! `components:publish` grant reaches the registry in its own org and nothing
//! else, a token without it reaches nothing, and another org's registry is
//! out of reach either way.
//!
//! Tokens are minted through `IssueClientCredentialsToken`, which is
//! service-account-gated (Forage calls it), so that one call goes through
//! `restricted_fixture()`. Both fixtures share the database.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{
    Fixture, RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY, fixture, restricted_fixture,
};

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

async fn registered_user(fixture: &Fixture) -> String {
    fixture
        .users()
        .register(RegisterRequest {
            username: format!("oct-user-{}", uuid::Uuid::now_v7()),
            email: format!("oct-{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register")
        .into_inner()
        .tokens
        .expect("tokens")
        .access_token
}

/// Create an organisation owned by `token`'s user; returns (id, name).
async fn create_org(fixture: &Fixture, token: &str) -> (String, String) {
    let name = format!("oct-org-{}", uuid::Uuid::now_v7());
    fixture
        .organisations()
        .create_organisation(authed(
            token,
            CreateOrganisationRequest { name: name.clone() },
        ))
        .await
        .expect("create org");
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM organisations WHERE name = $1")
        .bind(&name)
        .fetch_one(&fixture.db)
        .await
        .expect("org id");
    (id.to_string(), name)
}

/// A machine app in `org_id`, registered for both machine scopes.
async fn machine_app(fixture: &Fixture, token: &str, org_id: &str) -> (String, String) {
    let resp = fixture
        .oauth_apps()
        .create_o_auth_app(authed(
            token,
            CreateOAuthAppRequest {
                organisation_id: org_id.into(),
                name: "CI publisher".into(),
                description: String::new(),
                homepage_url: String::new(),
                redirect_uris: vec!["https://app.example/callback".into()],
                scopes: vec!["components:publish".into(), "directory:read".into()],
                grant_types: vec!["client_credentials".into()],
            },
        ))
        .await
        .expect("create app")
        .into_inner();
    (resp.app.expect("app").client_id, resp.client_secret)
}

async fn client_token(client_id: &str, client_secret: &str, scopes: &[&str]) -> String {
    restricted_fixture()
        .await
        .unwrap()
        .oauth_apps()
        .issue_client_credentials_token(authed(
            RESTRICTED_FIXTURE_SERVICE_ACCOUNT_KEY,
            IssueClientCredentialsTokenRequest {
                client_id: client_id.into(),
                client_secret: client_secret.into(),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
            },
        ))
        .await
        .expect("issue client token")
        .into_inner()
        .access_token
}

async fn begin_upload(fixture: &Fixture, token: &str, org: &str) -> Result<String, tonic::Status> {
    fixture
        .registry()
        .begin_upload(authed(
            token,
            BeginUploadRequest {
                name: format!("comp-{}", &uuid::Uuid::now_v7().simple().to_string()[..12]),
                organisation: org.into(),
                version: "0.1.0".into(),
            },
        ))
        .await
        .map(|r| r.into_inner().upload_context)
}

struct Setup {
    fx: Fixture,
    org_name: String,
    client_id: String,
    client_secret: String,
}

async fn setup() -> Setup {
    let fx = fixture().await.unwrap();
    let owner = registered_user(&fx).await;
    let (org_id, org_name) = create_org(&fx, &owner).await;
    let (client_id, client_secret) = machine_app(&fx, &owner, &org_id).await;
    Setup {
        fx,
        org_name,
        client_id,
        client_secret,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_token_can_begin_an_upload_in_its_own_organisation() {
    let s = setup().await;
    let token = client_token(&s.client_id, &s.client_secret, &["components:publish"]).await;

    let upload = begin_upload(&s.fx, &token, &s.org_name)
        .await
        .expect("a components:publish token may publish in its own org");
    assert!(!upload.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_without_the_publish_grant_cannot_publish() {
    let s = setup().await;
    let token = client_token(&s.client_id, &s.client_secret, &["directory:read"]).await;

    let err = begin_upload(&s.fx, &token, &s.org_name)
        .await
        .expect_err("directory:read must not reach the registry");
    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");
}

/// Another org's registry is out of reach even with the grant: the token is
/// its app's, and the app belongs to one organisation.
#[tokio::test(flavor = "multi_thread")]
async fn a_publish_token_cannot_publish_into_another_organisation() {
    let s = setup().await;
    let token = client_token(&s.client_id, &s.client_secret, &["components:publish"]).await;

    let other_owner = registered_user(&s.fx).await;
    let (_, other_org) = create_org(&s.fx, &other_owner).await;

    let err = begin_upload(&s.fx, &token, &other_org)
        .await
        .expect_err("publishing into another org must be refused");
    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");
}

/// The grant is for publishing, not for acting as an org member: a method
/// outside its list is refused even in the token's own organisation, where
/// an unscoped app would pass the org-member check.
#[tokio::test(flavor = "multi_thread")]
async fn a_publish_token_cannot_call_methods_outside_publishing() {
    let s = setup().await;
    let token = client_token(&s.client_id, &s.client_secret, &["components:publish"]).await;

    // GetDestinations needs only org membership, which an unscoped app in
    // its own org has: this is refused by the scope list and nothing else.
    let err =
        s.fx.destinations()
            .get_destinations(authed(
                &token,
                GetDestinationsRequest {
                    organisation: s.org_name.clone(),
                },
            ))
            .await
            .expect_err("a publish token must not read the org's destinations");
    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");

    let err =
        s.fx.destinations()
            .create_destination(authed(&token, CreateDestinationRequest::default()))
            .await
            .expect_err("a publish token must not create destinations");
    assert_eq!(err.code(), tonic::Code::PermissionDenied, "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_token_is_still_unauthenticated() {
    let s = setup().await;
    let err = begin_upload(&s.fx, "not-a-real-token", &s.org_name)
        .await
        .expect_err("garbage must not authenticate");
    assert_eq!(err.code(), tonic::Code::Unauthenticated, "{err:?}");
}
