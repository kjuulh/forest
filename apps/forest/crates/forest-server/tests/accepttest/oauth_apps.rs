//! End-to-end tests for organisation-owned OAuth applications (M1).
//!
//! Exercises the full CRUD lifecycle through the gRPC surface against a real
//! database, plus authorization boundaries (non-admins / non-members are
//! denied) and input validation (redirect URIs, scopes).

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{Fixture, fixture};

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

async fn register_user(fixture: &Fixture) -> String {
    let mut users = fixture.users();
    let resp = users
        .register(RegisterRequest {
            username: format!("user-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");
    resp.into_inner().tokens.expect("tokens").access_token
}

/// Create an org as `token` (creator becomes admin) and return (name, id).
async fn create_org(fixture: &Fixture, token: &str) -> (String, String) {
    let name = format!("org-{}", uuid::Uuid::now_v7());
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
        .expect("resolve org id");
    (name, id.to_string())
}

fn default_create(org_id: &str) -> CreateOAuthAppRequest {
    CreateOAuthAppRequest {
        organisation_id: org_id.to_string(),
        name: "My Integration".into(),
        description: "Does useful things".into(),
        homepage_url: "https://app.example".into(),
        redirect_uris: vec!["https://app.example/callback".into()],
        scopes: vec!["profile".into(), "email".into()],
        grant_types: vec![],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn full_lifecycle_create_list_get_update_rotate_delete() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let (_org, org_id) = create_org(&fixture, &token).await;
    let mut client = fixture.oauth_apps();

    // Create
    let created = client
        .create_o_auth_app(authed(&token, default_create(&org_id)))
        .await
        .expect("create app")
        .into_inner();
    let app = created.app.expect("app");
    assert_eq!(app.name, "My Integration");
    assert_eq!(app.organisation_id, org_id);
    assert!(app.client_id.starts_with("forest_oa_"));
    assert_eq!(app.scopes, vec!["profile", "email"]);
    assert!(
        !created.client_secret.is_empty(),
        "secret returned on create"
    );
    assert!(created.client_secret.starts_with("forest_oas_"));
    let app_id = app.app_id.clone();

    // List — exactly one app, secret never exposed in the listing
    let listed = client
        .list_o_auth_apps(authed(
            &token,
            ListOAuthAppsRequest {
                organisation_id: org_id.clone(),
            },
        ))
        .await
        .expect("list apps")
        .into_inner();
    assert_eq!(listed.apps.len(), 1);
    assert_eq!(listed.apps[0].app_id, app_id);

    // Get
    let got = client
        .get_o_auth_app(authed(
            &token,
            GetOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id: app_id.clone(),
            },
        ))
        .await
        .expect("get app")
        .into_inner()
        .app
        .expect("app");
    assert_eq!(got.client_id, app.client_id);

    // Update
    let updated = client
        .update_o_auth_app(authed(
            &token,
            UpdateOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id: app_id.clone(),
                name: "Renamed".into(),
                description: "new desc".into(),
                homepage_url: "https://app.example".into(),
                redirect_uris: vec!["https://app.example/cb2".into()],
                scopes: vec!["profile".into()],
                grant_types: vec![],
            },
        ))
        .await
        .expect("update app")
        .into_inner()
        .app
        .expect("app");
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.redirect_uris, vec!["https://app.example/cb2"]);
    assert_eq!(updated.scopes, vec!["profile"]);
    assert_eq!(updated.client_id, app.client_id, "client_id is stable");

    // Rotate secret — new secret, same client_id
    let rotated = client
        .rotate_o_auth_app_secret(authed(
            &token,
            RotateOAuthAppSecretRequest {
                organisation_id: org_id.clone(),
                app_id: app_id.clone(),
            },
        ))
        .await
        .expect("rotate secret")
        .into_inner();
    assert!(rotated.client_secret.starts_with("forest_oas_"));
    assert_ne!(rotated.client_secret, created.client_secret);
    assert_eq!(rotated.app.expect("app").client_id, app.client_id);

    // Delete, then get → not found
    client
        .delete_o_auth_app(authed(
            &token,
            DeleteOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id: app_id.clone(),
            },
        ))
        .await
        .expect("delete app");

    let err = client
        .get_o_auth_app(authed(
            &token,
            GetOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id: app_id.clone(),
            },
        ))
        .await
        .expect_err("app should be gone");
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn non_member_cannot_manage_apps() {
    let fixture = fixture().await.expect("fixture");
    let owner = register_user(&fixture).await;
    let (_org, org_id) = create_org(&fixture, &owner).await;

    // An app belonging to the org.
    let app = fixture
        .oauth_apps()
        .create_o_auth_app(authed(&owner, default_create(&org_id)))
        .await
        .expect("create app")
        .into_inner()
        .app
        .expect("app");

    // A different user who is not a member of the org.
    let outsider = register_user(&fixture).await;
    let mut client = fixture.oauth_apps();

    let list_err = client
        .list_o_auth_apps(authed(
            &outsider,
            ListOAuthAppsRequest {
                organisation_id: org_id.clone(),
            },
        ))
        .await
        .expect_err("outsider denied");
    assert_eq!(list_err.code(), tonic::Code::PermissionDenied);

    let create_err = client
        .create_o_auth_app(authed(&outsider, default_create(&org_id)))
        .await
        .expect_err("outsider denied");
    assert_eq!(create_err.code(), tonic::Code::PermissionDenied);

    let delete_err = client
        .delete_o_auth_app(authed(
            &outsider,
            DeleteOAuthAppRequest {
                organisation_id: org_id.clone(),
                app_id: app.app_id.clone(),
            },
        ))
        .await
        .expect_err("outsider denied");
    assert_eq!(delete_err.code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_is_rejected() {
    let fixture = fixture().await.expect("fixture");
    let (_org, org_id) = {
        let token = register_user(&fixture).await;
        create_org(&fixture, &token).await
    };

    let err = fixture
        .oauth_apps()
        .list_o_auth_apps(tonic::Request::new(ListOAuthAppsRequest {
            organisation_id: org_id,
        }))
        .await
        .expect_err("no auth");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_redirect_uri_and_scope_are_rejected() {
    let fixture = fixture().await.expect("fixture");
    let token = register_user(&fixture).await;
    let (_org, org_id) = create_org(&fixture, &token).await;
    let mut client = fixture.oauth_apps();

    // Non-loopback http redirect → invalid_argument.
    let mut bad_uri = default_create(&org_id);
    bad_uri.redirect_uris = vec!["http://app.example/cb".into()];
    let err = client
        .create_o_auth_app(authed(&token, bad_uri))
        .await
        .expect_err("bad redirect");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // Unknown scope → invalid_argument.
    let mut bad_scope = default_create(&org_id);
    bad_scope.scopes = vec!["profile".into(), "admin".into()];
    let err = client
        .create_o_auth_app(authed(&token, bad_scope))
        .await
        .expect_err("bad scope");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}
