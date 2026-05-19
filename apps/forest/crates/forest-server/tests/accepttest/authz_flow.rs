//! Authorization boundary tests.
//!
//! These tests verify that authenticated users CANNOT access resources
//! belonging to organisations they are not a member of.

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::fixture;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

fn unauthed_request<T>(inner: T) -> tonic::Request<T> {
    tonic::Request::new(inner)
}

/// Register a new user and return their access token.
async fn register_user(fixture: &crate::accepttest::fixtures::Fixture) -> String {
    let mut users = fixture.users();
    let username = format!("user-{}", uuid::Uuid::now_v7());
    let email = format!("{}@test.com", uuid::Uuid::now_v7());

    let resp = users
        .register(RegisterRequest {
            username,
            email,
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");

    resp.into_inner().tokens.expect("tokens").access_token
}

/// Create an organisation and return its name.
async fn create_org(
    fixture: &crate::accepttest::fixtures::Fixture,
    token: &str,
) -> String {
    let mut org_client = fixture.organisations();
    let name = format!("org-{}", uuid::Uuid::now_v7());

    org_client
        .create_organisation(authed_request(token, CreateOrganisationRequest { name: name.clone() }))
        .await
        .expect("create org");

    name
}

/// Set up a full org with environment + destination, return (org, env, dest) names.
async fn setup_org_with_destination(
    fixture: &crate::accepttest::fixtures::Fixture,
    token: &str,
) -> (String, String, String) {
    let org = create_org(fixture, token).await;

    let env_name = format!("env-{}", uuid::Uuid::now_v7());
    fixture
        .environments()
        .create_environment(authed_request(
            token,
            CreateEnvironmentRequest {
                organisation: org.clone(),
                name: env_name.clone(),
                description: None,
                sort_order: 0,
            },
        ))
        .await
        .expect("create env");

    let dest_name = format!("dest-{}", uuid::Uuid::now_v7());
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("cluster_name".into(), "test".into());
    metadata.insert("namespace".into(), "test".into());
    metadata.insert("local_path".into(), format!("/tmp/authz-test-{}", uuid::Uuid::now_v7()));

    fixture
        .destinations()
        .create_destination(authed_request(
            token,
            CreateDestinationRequest {
                organisation: org.clone(),
                name: dest_name.clone(),
                environment: env_name.clone(),
                metadata,
                r#type: Some(DestinationType {
                    organisation: "forest".into(),
                    name: "flux".into(),
                    version: 1,
                    description: String::new(),
                    fields: vec![],
                }),
            },
        ))
        .await
        .expect("create dest");

    (org, env_name, dest_name)
}

// ═══════════════════════════════════════════════════════════════════════
// Tests: Unauthenticated access
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_cannot_list_projects() {
    let fixture = fixture().await.unwrap();
    let mut client = fixture.releases();

    let result = client
        .get_projects(unauthed_request(GetProjectsRequest {
            query: Some(get_projects_request::Query::Organisation(OrganisationRef {
                organisation: "anything".into(),
            })),
        }))
        .await;

    assert!(
        result.is_err(),
        "unauthenticated request should be rejected, got: {:?}",
        result,
    );
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_cannot_create_destination() {
    let fixture = fixture().await.unwrap();
    let mut client = fixture.destinations();

    let result = client
        .create_destination(unauthed_request(CreateDestinationRequest {
            organisation: "anything".into(),
            name: "rogue-dest".into(),
            environment: "prod".into(),
            metadata: Default::default(),
            r#type: Some(DestinationType {
                organisation: "forest".into(),
                name: "flux".into(),
                version: 1,
                description: String::new(),
                fields: vec![],
            }),
        }))
        .await;

    assert!(
        result.is_err(),
        "unauthenticated request should be rejected",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
}

// ═══════════════════════════════════════════════════════════════════════
// Tests: Cross-org access (user B tries to access org A's resources)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_list_projects_in_other_org() {
    let fixture = fixture().await.unwrap();

    // User A creates an org with a project
    let token_a = register_user(&fixture).await;
    let org_a = create_org(&fixture, &token_a).await;

    // Create a project in org A
    fixture
        .releases()
        .create_project(authed_request(
            &token_a,
            CreateProjectRequest {
                organisation: org_a.clone(),
                project: "secret-project".into(),
            },
        ))
        .await
        .expect("create project");

    // User B (not a member of org A) tries to list org A's projects
    let token_b = register_user(&fixture).await;

    let result = fixture
        .releases()
        .get_projects(authed_request(
            &token_b,
            GetProjectsRequest {
                query: Some(get_projects_request::Query::Organisation(OrganisationRef {
                    organisation: org_a.clone(),
                })),
            },
        ))
        .await;

    // This SHOULD fail with PermissionDenied, but currently returns data
    match result {
        Ok(resp) => {
            let projects = resp.into_inner().projects;
            assert!(
                projects.is_empty(),
                "SECURITY: user B can see org A's projects: {:?}",
                projects,
            );
        }
        Err(status) => {
            assert_eq!(
                status.code(),
                tonic::Code::PermissionDenied,
                "expected PermissionDenied, got: {}",
                status,
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_create_destination_in_other_org() {
    let fixture = fixture().await.unwrap();

    // User A creates an org with an environment
    let token_a = register_user(&fixture).await;
    let org_a = create_org(&fixture, &token_a).await;
    let env_name = format!("env-{}", uuid::Uuid::now_v7());

    fixture
        .environments()
        .create_environment(authed_request(
            &token_a,
            CreateEnvironmentRequest {
                organisation: org_a.clone(),
                name: env_name.clone(),
                description: None,
                sort_order: 0,
            },
        ))
        .await
        .expect("create env");

    // User B tries to create a destination in org A
    let token_b = register_user(&fixture).await;

    let result = fixture
        .destinations()
        .create_destination(authed_request(
            &token_b,
            CreateDestinationRequest {
                organisation: org_a.clone(),
                name: format!("rogue-dest-{}", uuid::Uuid::now_v7()),
                environment: env_name,
                metadata: Default::default(),
                r#type: Some(DestinationType {
                    organisation: "forest".into(),
                    name: "flux".into(),
                    version: 1,
                    description: String::new(),
                    fields: vec![],
                }),
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B created a destination in org A's environment",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_create_environment_in_other_org() {
    let fixture = fixture().await.unwrap();

    // User A creates an org
    let token_a = register_user(&fixture).await;
    let org_a = create_org(&fixture, &token_a).await;

    // User B tries to create an environment in org A
    let token_b = register_user(&fixture).await;

    let result = fixture
        .environments()
        .create_environment(authed_request(
            &token_b,
            CreateEnvironmentRequest {
                organisation: org_a.clone(),
                name: format!("rogue-env-{}", uuid::Uuid::now_v7()),
                description: None,
                sort_order: 0,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B created an environment in org A",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_delete_destination_in_other_org() {
    let fixture = fixture().await.unwrap();

    // User A sets up org with destination
    let token_a = register_user(&fixture).await;
    let (org_a, _env, dest) = setup_org_with_destination(&fixture, &token_a).await;

    // User B tries to delete it
    let token_b = register_user(&fixture).await;

    // DeleteDestinationRequest only takes `name` (globally unique) — no org param!
    // This means ANY authenticated user can delete ANY destination by name.
    let result = fixture
        .destinations()
        .delete_destination(authed_request(
            &token_b,
            DeleteDestinationRequest { name: dest },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B deleted org A's destination by name alone",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_release_to_other_orgs_destination() {
    let fixture = fixture().await.unwrap();

    // User A sets up org with destination and an annotated artifact
    let token_a = register_user(&fixture).await;
    let (org_a, _env, _dest) = setup_org_with_destination(&fixture, &token_a).await;

    // Create project + artifact in org A
    fixture
        .releases()
        .create_project(authed_request(
            &token_a,
            CreateProjectRequest {
                organisation: org_a.clone(),
                project: "proj-a".into(),
            },
        ))
        .await
        .expect("create project");

    // User B registers and tries to release to org A's destination
    let token_b = register_user(&fixture).await;

    // First, user B needs an artifact. Create their own org and artifact.
    let org_b = create_org(&fixture, &token_b).await;

    // User B tries to annotate a release in org A's project
    let result = fixture
        .releases()
        .annotate_release(authed_request(
            &token_b,
            AnnotateReleaseRequest {
                artifact_id: uuid::Uuid::now_v7().to_string(),
                project: Some(Project {
                    organisation: org_a.clone(),
                    project: "proj-a".into(),
                }),
                metadata: Default::default(),
                source: Some(Source {
                    user: Some("attacker".into()),
                    email: Some("attacker@evil.com".into()),
                    user_id: None,
                    source_type: Some("ci".into()),
                    run_url: None,
                }),
                context: Some(ArtifactContext {
                    title: "malicious release".into(),
                    description: None,
                    web: None,
                    pr: None,
                }),
                r#ref: Some(Ref {
                    commit_sha: "deadbeef".into(),
                    branch: Some("main".into()),
                    commit_message: None,
                    version: None,
                    repo_url: None,
                }),
                annotation_only: false,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B annotated a release in org A's project",
    );
    let _ = org_b; // used to create user B's org
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_list_triggers_in_other_org() {
    let fixture = fixture().await.unwrap();

    // User A creates org + project
    let token_a = register_user(&fixture).await;
    let org_a = create_org(&fixture, &token_a).await;

    fixture
        .releases()
        .create_project(authed_request(
            &token_a,
            CreateProjectRequest {
                organisation: org_a.clone(),
                project: "proj-triggers".into(),
            },
        ))
        .await
        .expect("create project");

    // User B tries to list triggers in org A's project
    let token_b = register_user(&fixture).await;

    let mut trigger_client =
        forest_grpc_interface::trigger_service_client::TriggerServiceClient::new(
            fixture.channel.clone(),
        );

    let result = trigger_client
        .list_triggers(authed_request(
            &token_b,
            ListTriggersRequest {
                project: Some(Project {
                    organisation: org_a,
                    project: "proj-triggers".into(),
                }),
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B should not be able to list triggers in org A's project",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_get_destination_states_for_other_org() {
    let fixture = fixture().await.unwrap();

    // User A creates org with destination
    let token_a = register_user(&fixture).await;
    let (org_a, _env, _dest) = setup_org_with_destination(&fixture, &token_a).await;

    // User B tries to get destination states for org A
    let token_b = register_user(&fixture).await;

    let result = fixture
        .releases()
        .get_destination_states(authed_request(
            &token_b,
            GetDestinationStatesRequest {
                organisation: org_a,
                project: None,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B should not be able to view org A's destination states",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_list_environments_in_other_org() {
    let fixture = fixture().await.unwrap();

    let token_a = register_user(&fixture).await;
    let (org_a, _env, _dest) = setup_org_with_destination(&fixture, &token_a).await;

    let token_b = register_user(&fixture).await;

    let result = fixture
        .environments()
        .list_environments(authed_request(
            &token_b,
            ListEnvironmentsRequest {
                organisation: org_a,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B should not be able to list org A's environments",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_list_destinations_in_other_org() {
    let fixture = fixture().await.unwrap();

    let token_a = register_user(&fixture).await;
    let (org_a, _env, _dest) = setup_org_with_destination(&fixture, &token_a).await;

    let token_b = register_user(&fixture).await;

    let result = fixture
        .destinations()
        .get_destinations(authed_request(
            &token_b,
            GetDestinationsRequest {
                organisation: org_a,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B should not be able to list org A's destinations",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn user_cannot_get_release_intent_states_for_other_org() {
    let fixture = fixture().await.unwrap();

    let token_a = register_user(&fixture).await;
    let org_a = create_org(&fixture, &token_a).await;

    let token_b = register_user(&fixture).await;

    let result = fixture
        .releases()
        .get_release_intent_states(authed_request(
            &token_b,
            GetReleaseIntentStatesRequest {
                organisation: org_a,
                project: None,
                include_completed: false,
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "SECURITY: user B should not be able to view org A's release intent states",
    );
    assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
}
