//! Operator access to terraform@1 state (understory-io/forest#282): a
//! short-lived backend credential, versioned reads, and force-unlock — each
//! restricted to the organisation's admins.

use axum::body::{Body, to_bytes};
use base64::Engine as _;
use forest_grpc_interface::*;
use forest_server::destinations::terraformv1::{TerraformStateStoreState, TerraformV1Server};
use http::{Method, Request, StatusCode};
use tonic::metadata::MetadataValue;
use tower::ServiceExt;

use crate::accepttest::fixtures::{Fixture, fixture};

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {token}").parse().unwrap();
    req.metadata_mut().insert("authorization", val);
    req
}

async fn register_user(f: &Fixture) -> String {
    f.users()
        .register(RegisterRequest {
            username: format!("user-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .tokens
        .unwrap()
        .access_token
}

/// An org with a project and an environment, owned by a fresh user.
struct World {
    token: String,
    org: String,
    project: String,
    project_id: String,
    env: String,
}

async fn world(f: &Fixture) -> World {
    let token = register_user(f).await;
    let org = format!("org-{}", uuid::Uuid::now_v7());
    f.organisations()
        .create_organisation(authed(
            &token,
            CreateOrganisationRequest { name: org.clone() },
        ))
        .await
        .unwrap();
    let project = "svc".to_string();
    f.releases()
        .create_project(authed(
            &token,
            CreateProjectRequest {
                organisation: org.clone(),
                project: project.clone(),
            },
        ))
        .await
        .unwrap();
    let env = format!("env-{}", uuid::Uuid::now_v7());
    f.environments()
        .create_environment(authed(
            &token,
            CreateEnvironmentRequest {
                organisation: org.clone(),
                name: env.clone(),
                description: None,
                sort_order: 0,
            },
        ))
        .await
        .unwrap();
    let project_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM projects WHERE organisation = $1 AND project = $2")
            .bind(&org)
            .bind(&project)
            .fetch_one(&f.db)
            .await
            .unwrap();
    World {
        token,
        org,
        project,
        project_id: project_id.to_string(),
        env,
    }
}

async fn terraform_destination(f: &Fixture, w: &World, name: &str) {
    f.destinations()
        .create_destination(authed(
            &w.token,
            CreateDestinationRequest {
                organisation: w.org.clone(),
                name: name.to_string(),
                environment: w.env.clone(),
                metadata: Default::default(),
                r#type: Some(DestinationType {
                    organisation: "forest".into(),
                    name: "terraform".into(),
                    version: 1,
                    description: String::new(),
                    fields: vec![],
                }),
                sensitive_keys: vec![],
            },
        ))
        .await
        .unwrap();
}

fn state_ref(w: &World, destination: &str) -> Option<TerraformStateRef> {
    Some(TerraformStateRef {
        organisation: w.org.clone(),
        project: w.project.clone(),
        environment: w.env.clone(),
        destination: destination.to_string(),
    })
}

async fn backend(
    f: &Fixture,
    method: Method,
    uri: &str,
    password: &str,
    body: &str,
) -> (StatusCode, String) {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("forest-terraform-v1:{password}"))
    );
    let response = TerraformV1Server::router(f.state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(http::header::AUTHORIZATION, auth)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_admin_gets_a_credential_that_works_against_the_backend() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-only").await;

    // Read-only by default.
    let ro = f
        .terraform_state()
        .issue_terraform_state_credential(authed(
            &w.token,
            IssueTerraformStateCredentialRequest {
                state: state_ref(&w, ""),
                write: false,
                ttl_seconds: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(!ro.write);
    assert!(
        ro.address.ends_with(&format!("/{}", ro.state_id)),
        "{}",
        ro.address
    );
    assert!(ro.expires_at_unix > 0);

    let path = format!("/{}", ro.state_id);
    let (status, _) = backend(&f, Method::GET, &path, &ro.password, "").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "authorised, nothing stored yet"
    );
    let (status, _) = backend(
        &f,
        Method::POST,
        &format!("{path}/lock"),
        &ro.password,
        r#"{"ID":"x"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "read-only cannot lock");

    let rw = f
        .terraform_state()
        .issue_terraform_state_credential(authed(
            &w.token,
            IssueTerraformStateCredentialRequest {
                state: state_ref(&w, ""),
                write: true,
                ttl_seconds: 60,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(rw.state_id, ro.state_id);
    let (status, _) = backend(
        &f,
        Method::POST,
        &format!("{path}/lock"),
        &rw.password,
        r#"{"ID":"op"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read-write can lock");
    f.state
        .terraform_state_store()
        .force_unlock(&rw.state_id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn another_organisation_cannot_touch_the_state() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-only").await;
    let outsider = register_user(&f).await;

    let issued = f
        .terraform_state()
        .issue_terraform_state_credential(authed(
            &outsider,
            IssueTerraformStateCredentialRequest {
                state: state_ref(&w, ""),
                write: true,
                ttl_seconds: 0,
            },
        ))
        .await;
    assert!(issued.is_err(), "outsider must not get a credential");

    let read = f
        .terraform_state()
        .get_terraform_state(authed(
            &outsider,
            GetTerraformStateRequest {
                state: state_ref(&w, ""),
                version: 0,
            },
        ))
        .await;
    assert!(read.is_err());

    let unlock = f
        .terraform_state()
        .force_unlock_terraform_state(authed(
            &outsider,
            ForceUnlockTerraformStateRequest {
                state: state_ref(&w, ""),
            },
        ))
        .await;
    assert!(unlock.is_err());

    // Nor with no token at all.
    let anonymous = f
        .terraform_state()
        .get_terraform_state(tonic::Request::new(GetTerraformStateRequest {
            state: state_ref(&w, ""),
            version: 0,
        }))
        .await;
    assert!(anonymous.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_are_readable_latest_first_and_by_number() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-only").await;
    // One terraform destination: the legacy id, as a run would resolve it.
    let state_id = forest_server::destinations::terraformv1::TerraformStateStore::legacy_state_id(
        &w.env,
        &w.project_id,
    );
    for serial in [1, 2] {
        sqlx::query("INSERT INTO terraform_states (state_id, content) VALUES ($1, $2)")
            .bind(&state_id)
            .bind(format!(r#"{{"serial":{serial}}}"#))
            .execute(&f.db)
            .await
            .unwrap();
    }

    let listed = f
        .terraform_state()
        .list_terraform_state_versions(authed(
            &w.token,
            ListTerraformStateVersionsRequest {
                state: state_ref(&w, ""),
                limit: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.state_id, state_id);
    assert_eq!(listed.versions.len(), 2);
    assert!(
        listed.versions[0].version > listed.versions[1].version,
        "newest first"
    );

    let latest = f
        .terraform_state()
        .get_terraform_state(authed(
            &w.token,
            GetTerraformStateRequest {
                state: state_ref(&w, ""),
                version: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(latest.content, r#"{"serial":2}"#);

    let older = f
        .terraform_state()
        .get_terraform_state(authed(
            &w.token,
            GetTerraformStateRequest {
                state: state_ref(&w, ""),
                version: listed.versions[1].version,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(older.content, r#"{"serial":1}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn several_terraform_destinations_need_one_named() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-a").await;
    terraform_destination(&f, &w, "tf-b").await;
    // tf-a has run, so it owns the legacy id; tf-b has one of its own.
    let legacy = forest_server::destinations::terraformv1::TerraformStateStore::legacy_state_id(
        &w.env,
        &w.project_id,
    );
    sqlx::query(
        "INSERT INTO terraform_state_bindings (state_id, organisation, destination) VALUES ($1, $2, 'tf-a')",
    )
    .bind(&legacy)
    .bind(&w.org)
    .execute(&f.db)
    .await
    .unwrap();

    let versions = async |destination: &str| {
        f.terraform_state()
            .list_terraform_state_versions(authed(
                &w.token,
                ListTerraformStateVersionsRequest {
                    state: state_ref(&w, destination),
                    limit: 0,
                },
            ))
            .await
    };

    let ambiguous = versions("").await.unwrap_err();
    assert_eq!(ambiguous.code(), tonic::Code::FailedPrecondition);
    assert!(
        ambiguous.message().contains("tf-a, tf-b"),
        "{}",
        ambiguous.message()
    );

    assert_eq!(
        versions("tf-a").await.unwrap().into_inner().state_id,
        legacy
    );
    assert_eq!(
        versions("tf-b").await.unwrap().into_inner().state_id,
        forest_server::destinations::terraformv1::TerraformStateStore::destination_state_id(
            &w.env,
            &w.project_id,
            "tf-b"
        )
    );

    let missing = versions("tf-nope").await.unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_state_is_found_without_naming_a_destination_and_is_not_bound_by_looking() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-a").await;
    terraform_destination(&f, &w, "tf-b").await;
    let legacy = forest_server::destinations::terraformv1::TerraformStateStore::legacy_state_id(
        &w.env,
        &w.project_id,
    );
    sqlx::query("INSERT INTO terraform_states (state_id, content) VALUES ($1, '{}')")
        .bind(&legacy)
        .execute(&f.db)
        .await
        .unwrap();

    let listed = f
        .terraform_state()
        .list_terraform_state_versions(authed(
            &w.token,
            ListTerraformStateVersionsRequest {
                state: state_ref(&w, ""),
                limit: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.state_id, legacy);

    let bound: Option<String> =
        sqlx::query_scalar("SELECT destination FROM terraform_state_bindings WHERE state_id = $1")
            .bind(&legacy)
            .fetch_optional(&f.db)
            .await
            .unwrap();
    assert_eq!(bound, None, "an operator reading must not choose the owner");
}

#[tokio::test(flavor = "multi_thread")]
async fn force_unlock_reports_what_it_removed() {
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-only").await;
    // One terraform destination: the legacy id, as a run would resolve it.
    let state_id = forest_server::destinations::terraformv1::TerraformStateStore::legacy_state_id(
        &w.env,
        &w.project_id,
    );
    sqlx::query(
        "INSERT INTO terraform_state_locks (state_id, lock_id) VALUES ($1, 'crashed-apply')",
    )
    .bind(&state_id)
    .execute(&f.db)
    .await
    .unwrap();

    let listed = f
        .terraform_state()
        .list_terraform_state_versions(authed(
            &w.token,
            ListTerraformStateVersionsRequest {
                state: state_ref(&w, ""),
                limit: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.lock_id, "crashed-apply");

    let unlocked = f
        .terraform_state()
        .force_unlock_terraform_state(authed(
            &w.token,
            ForceUnlockTerraformStateRequest {
                state: state_ref(&w, ""),
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(unlocked.removed_lock_id, "crashed-apply");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_who_is_not_an_admin_is_refused_every_call() {
    // Admin only, reads included: a pulled state carries secrets.
    let f = fixture().await.unwrap();
    let w = world(&f).await;
    terraform_destination(&f, &w, "tf-only").await;

    let member = f
        .users()
        .register(RegisterRequest {
            username: format!("member-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .unwrap()
        .into_inner();
    let member_token = member.tokens.unwrap().access_token;
    let org_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM organisations WHERE name = $1")
        .bind(&w.org)
        .fetch_one(&f.db)
        .await
        .unwrap();
    f.organisations()
        .add_member(authed(
            &w.token,
            AddMemberRequest {
                organisation_id: org_id.to_string(),
                user_id: member.user.unwrap().user_id,
                role: "member".into(),
            },
        ))
        .await
        .unwrap();

    let mut client = f.terraform_state();
    let denied = |r: Result<(), tonic::Status>, what: &str| {
        let status = r.expect_err(what);
        assert_eq!(status.code(), tonic::Code::PermissionDenied, "{what}");
        assert!(
            status.message().contains("admin"),
            "{what}: {}",
            status.message()
        );
    };

    for write in [false, true] {
        denied(
            client
                .issue_terraform_state_credential(authed(
                    &member_token,
                    IssueTerraformStateCredentialRequest {
                        state: state_ref(&w, ""),
                        write,
                        ttl_seconds: 0,
                    },
                ))
                .await
                .map(|_| ()),
            "credential",
        );
    }
    denied(
        client
            .get_terraform_state(authed(
                &member_token,
                GetTerraformStateRequest {
                    state: state_ref(&w, ""),
                    version: 0,
                },
            ))
            .await
            .map(|_| ()),
        "pull",
    );
    denied(
        client
            .list_terraform_state_versions(authed(
                &member_token,
                ListTerraformStateVersionsRequest {
                    state: state_ref(&w, ""),
                    limit: 0,
                },
            ))
            .await
            .map(|_| ()),
        "versions",
    );
    denied(
        client
            .force_unlock_terraform_state(authed(
                &member_token,
                ForceUnlockTerraformStateRequest {
                    state: state_ref(&w, ""),
                },
            ))
            .await
            .map(|_| ()),
        "unlock",
    );
}
