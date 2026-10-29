use forest_grpc_interface::*;
use futures::StreamExt;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{Fixture, fixture};

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

struct User {
    id: uuid::Uuid,
    token: String,
}

async fn register_user(fixture: &Fixture) -> User {
    let resp = fixture
        .users()
        .register(RegisterRequest {
            username: format!("user-{}", uuid::Uuid::now_v7()),
            email: format!("{}@test.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user")
        .into_inner();

    User {
        id: resp.user.expect("user").user_id.parse().expect("user id"),
        token: resp.tokens.expect("tokens").access_token,
    }
}

async fn create_org(fixture: &Fixture, owner: &User) -> String {
    let name = format!("org-{}", uuid::Uuid::now_v7());
    fixture
        .organisations()
        .create_organisation(authed_request(
            &owner.token,
            CreateOrganisationRequest { name: name.clone() },
        ))
        .await
        .expect("create org");
    name
}

async fn publish(fixture: &Fixture, owner: &User, organisation: &str, name: &str, version: &str) {
    let _ = fixture
        .releases()
        .create_project(authed_request(
            &owner.token,
            CreateProjectRequest {
                organisation: organisation.into(),
                project: name.into(),
            },
        ))
        .await;

    let mut registry = fixture.registry();
    let upload_context = registry
        .begin_upload(authed_request(
            &owner.token,
            BeginUploadRequest {
                name: name.into(),
                organisation: organisation.into(),
                version: version.into(),
            },
        ))
        .await
        .expect("begin upload")
        .into_inner()
        .upload_context;

    registry
        .upload_file(authed_request(
            &owner.token,
            UploadFileRequest {
                upload_context: upload_context.clone(),
                file_path: "forest.component.cue".into(),
                file_content: b"package component\n".to_vec(),
            },
        ))
        .await
        .expect("upload file");

    registry
        .commit_upload(authed_request(
            &owner.token,
            CommitUploadRequest { upload_context },
        ))
        .await
        .expect("commit upload");
}

async fn set_visibility(
    fixture: &Fixture,
    token: &str,
    organisation: &str,
    name: &str,
    visibility: &str,
) -> Result<(), tonic::Status> {
    fixture
        .releases()
        .update_project(authed_request(
            token,
            UpdateProjectRequest {
                organisation: organisation.into(),
                project: name.into(),
                readme: None,
                description: None,
                metadata: None,
                visibility: Some(visibility.into()),
            },
        ))
        .await
        .map(|_| ())
}

async fn add_member(fixture: &Fixture, organisation: &str, user: &User) {
    sqlx::query(
        "INSERT INTO organisation_members (organisation_id, user_id, role)
         SELECT id, $2, 'member' FROM organisations WHERE name = $1",
    )
    .bind(organisation)
    .bind(user.id)
    .execute(&fixture.db)
    .await
    .expect("add member");
}

async fn resolve_as_consumer(
    fixture: &Fixture,
    token: &str,
    organisation: &str,
    name: &str,
    version: &str,
) -> Result<usize, tonic::Status> {
    let mut registry = fixture.registry();

    let component = registry
        .get_component_version(authed_request(
            token,
            GetComponentVersionRequest {
                name: name.into(),
                organisation: organisation.into(),
                version: version.into(),
            },
        ))
        .await?
        .into_inner()
        .component
        .expect("component version exists");

    let mut stream = registry
        .get_component_files(authed_request(
            token,
            GetComponentFilesRequest {
                component_id: component.id,
            },
        ))
        .await?
        .into_inner();
    let mut files = 0;
    while let Some(msg) = stream.next().await {
        msg?;
        files += 1;
    }

    registry
        .list_component_versions(authed_request(
            token,
            ListComponentVersionsRequest {
                organisation: organisation.into(),
                name: name.into(),
            },
        ))
        .await?;

    registry
        .get_component(authed_request(
            token,
            GetComponentRequest {
                name: name.into(),
                organisation: organisation.into(),
            },
        ))
        .await?;

    match registry
        .get_component_manifest(authed_request(
            token,
            GetComponentManifestRequest {
                organisation: organisation.into(),
                name: name.into(),
                version: version.into(),
            },
        ))
        .await
    {
        Ok(_) => {}
        Err(status) if status.code() == tonic::Code::NotFound => {}
        Err(status) => return Err(status),
    }

    registry
        .get_component_detail(authed_request(
            token,
            GetComponentDetailRequest {
                organisation: organisation.into(),
                name: name.into(),
            },
        ))
        .await?;

    let binary = registry
        .download_binary(authed_request(
            token,
            DownloadBinaryRequest {
                organisation: organisation.into(),
                name: name.into(),
                version: version.into(),
                os: "linux".into(),
                arch: "amd64".into(),
                ..Default::default()
            },
        ))
        .await;
    match binary {
        Ok(_) => {}
        Err(status) if status.code() == tonic::Code::NotFound => {}
        Err(status) => return Err(status),
    }

    Ok(files)
}

fn test_version() -> String {
    format!("0.0.1-test.{}", uuid::Uuid::now_v7().simple())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_resolves_their_own_private_component() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let version = test_version();
    publish(&fixture, &owner, &org, "widget", &version).await;

    let files = resolve_as_consumer(&fixture, &owner.token, &org, "widget", &version)
        .await
        .expect("an owner reads its own component");
    assert!(files > 0, "the owner should receive the component's files");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_member_cannot_resolve_a_private_component() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let version = test_version();
    publish(&fixture, &owner, &org, "secret", &version).await;
    let outsider = register_user(&fixture).await;

    let mut registry = fixture.registry();
    let refused = registry
        .get_component_version(authed_request(
            &outsider.token,
            GetComponentVersionRequest {
                name: "secret".into(),
                organisation: org.clone(),
                version: version.clone(),
            },
        ))
        .await
        .expect_err("a non-member must not read a private component");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let component_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM components WHERE organisation = $1 AND name = 'secret' AND version = $2",
    )
    .bind(&org)
    .bind(&version)
    .fetch_one(&fixture.db)
    .await
    .expect("component row");

    let refused = registry
        .get_component_files(authed_request(
            &outsider.token,
            GetComponentFilesRequest {
                component_id: component_id.to_string(),
            },
        ))
        .await
        .expect_err("a non-member must not read a private component's files by id");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    for refused in [
        registry
            .get_component(authed_request(
                &outsider.token,
                GetComponentRequest {
                    name: "secret".into(),
                    organisation: org.clone(),
                },
            ))
            .await
            .map(|_| ()),
        registry
            .list_component_versions(authed_request(
                &outsider.token,
                ListComponentVersionsRequest {
                    organisation: org.clone(),
                    name: "secret".into(),
                },
            ))
            .await
            .map(|_| ()),
        registry
            .get_component_manifest(authed_request(
                &outsider.token,
                GetComponentManifestRequest {
                    organisation: org.clone(),
                    name: "secret".into(),
                    version: version.clone(),
                },
            ))
            .await
            .map(|_| ()),
        registry
            .get_component_detail(authed_request(
                &outsider.token,
                GetComponentDetailRequest {
                    organisation: org.clone(),
                    name: "secret".into(),
                },
            ))
            .await
            .map(|_| ()),
        registry
            .download_binary(authed_request(
                &outsider.token,
                DownloadBinaryRequest {
                    organisation: org.clone(),
                    name: "secret".into(),
                    version: version.clone(),
                    os: "linux".into(),
                    arch: "amd64".into(),
                    ..Default::default()
                },
            ))
            .await
            .map(|_| ()),
        registry
            .list_org_tools(authed_request(
                &outsider.token,
                ListOrgToolsRequest {
                    organisation: org.clone(),
                },
            ))
            .await
            .map(|_| ()),
    ] {
        assert_eq!(
            refused.expect_err("a non-member is refused").code(),
            tonic::Code::PermissionDenied
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_member_resolves_a_public_component() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let version = test_version();
    publish(&fixture, &owner, &org, "shared", &version).await;
    set_visibility(&fixture, &owner.token, &org, "shared", "public")
        .await
        .expect("an org admin makes its component public");
    let outsider = register_user(&fixture).await;

    let files = resolve_as_consumer(&fixture, &outsider.token, &org, "shared", &version)
        .await
        .expect("a public component is readable by any signed-in user");
    assert!(files > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn making_a_component_private_again_refuses_non_members() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let version = test_version();
    publish(&fixture, &owner, &org, "widget", &version).await;
    set_visibility(&fixture, &owner.token, &org, "widget", "public")
        .await
        .unwrap();
    set_visibility(&fixture, &owner.token, &org, "widget", "private")
        .await
        .unwrap();
    let outsider = register_user(&fixture).await;

    let refused = resolve_as_consumer(&fixture, &outsider.token, &org, "widget", &version)
        .await
        .expect_err("a private component is refused again");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_org_admin_changes_visibility() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    publish(&fixture, &owner, &org, "widget", &test_version()).await;
    let member = register_user(&fixture).await;
    add_member(&fixture, &org, &member).await;
    let outsider = register_user(&fixture).await;

    let refused = set_visibility(&fixture, &member.token, &org, "widget", "public")
        .await
        .expect_err("a plain member cannot publish the org's component to everyone");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let refused = set_visibility(&fixture, &outsider.token, &org, "widget", "public")
        .await
        .expect_err("a non-member cannot make another org's component public");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let still_private: String = sqlx::query_scalar(
        "SELECT visibility FROM projects WHERE organisation = $1 AND project = 'widget'",
    )
    .bind(&org)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(still_private, "private");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_still_updates_project_details_without_touching_visibility() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    publish(&fixture, &owner, &org, "widget", &test_version()).await;
    set_visibility(&fixture, &owner.token, &org, "widget", "public")
        .await
        .unwrap();
    let member = register_user(&fixture).await;
    add_member(&fixture, &org, &member).await;

    fixture
        .releases()
        .update_project(authed_request(
            &member.token,
            UpdateProjectRequest {
                organisation: org.clone(),
                project: "widget".into(),
                readme: None,
                description: Some("republished".into()),
                metadata: None,
                visibility: None,
            },
        ))
        .await
        .expect("publishing's project update needs no admin");

    let visibility: String = sqlx::query_scalar(
        "SELECT visibility FROM projects WHERE organisation = $1 AND project = 'widget'",
    )
    .bind(&org)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        visibility, "public",
        "a republish must not make it private again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_visibility_is_rejected() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    publish(&fixture, &owner, &org, "widget", &test_version()).await;

    let refused = set_visibility(&fixture, &owner.token, &org, "widget", "internal")
        .await
        .expect_err("only public and private exist");
    assert_eq!(refused.code(), tonic::Code::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_component_is_still_read_only_to_non_members() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let version = test_version();
    publish(&fixture, &owner, &org, "shared", &version).await;
    set_visibility(&fixture, &owner.token, &org, "shared", "public")
        .await
        .unwrap();
    let outsider = register_user(&fixture).await;
    let mut registry = fixture.registry();

    let owners_upload = registry
        .begin_upload(authed_request(
            &owner.token,
            BeginUploadRequest {
                name: "shared".into(),
                organisation: org.clone(),
                version: test_version(),
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .upload_context;

    let attempts: Vec<(&str, Result<(), tonic::Status>)> = vec![
        (
            "begin_upload",
            registry
                .begin_upload(authed_request(
                    &outsider.token,
                    BeginUploadRequest {
                        name: "shared".into(),
                        organisation: org.clone(),
                        version: test_version(),
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "upload_file into the owner's upload",
            registry
                .upload_file(authed_request(
                    &outsider.token,
                    UploadFileRequest {
                        upload_context: owners_upload.clone(),
                        file_path: "forest.component.cue".into(),
                        file_content: b"package hijacked\n".to_vec(),
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "publish_manifest into the owner's upload",
            registry
                .publish_manifest(authed_request(
                    &outsider.token,
                    PublishManifestRequest {
                        upload_context: owners_upload.clone(),
                        manifest_json: "{}".into(),
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "commit_upload of the owner's upload",
            registry
                .commit_upload(authed_request(
                    &outsider.token,
                    CommitUploadRequest {
                        upload_context: owners_upload.clone(),
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "unpublish_version",
            registry
                .unpublish_version(authed_request(
                    &outsider.token,
                    UnpublishVersionRequest {
                        organisation: org.clone(),
                        name: "shared".into(),
                        version: version.clone(),
                        reason: String::new(),
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "update_project details",
            fixture
                .releases()
                .update_project(authed_request(
                    &outsider.token,
                    UpdateProjectRequest {
                        organisation: org.clone(),
                        project: "shared".into(),
                        readme: Some("defaced".into()),
                        description: Some("defaced".into()),
                        metadata: None,
                        visibility: None,
                    },
                ))
                .await
                .map(|_| ()),
        ),
        (
            "update_project visibility",
            set_visibility(&fixture, &outsider.token, &org, "shared", "private").await,
        ),
    ];

    for (attempt, result) in attempts {
        let refused = result.expect_err(attempt);
        assert_eq!(
            refused.code(),
            tonic::Code::PermissionDenied,
            "{attempt}: {refused:?}"
        );
    }

    let (visibility, description): (String, String) = sqlx::query_as(
        "SELECT visibility, description FROM projects WHERE organisation = $1 AND project = 'shared'",
    )
    .bind(&org)
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(visibility, "public");
    assert_ne!(description, "defaced");
}

async fn set_org_visibility(
    fixture: &Fixture,
    token: &str,
    organisation: &str,
    visibility: &str,
) -> Result<(), tonic::Status> {
    fixture
        .organisations()
        .set_organisation_visibility(authed_request(
            token,
            SetOrganisationVisibilityRequest {
                organisation: organisation.into(),
                visibility: visibility.into(),
            },
        ))
        .await
        .map(|_| ())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_organisation_shares_every_component_read_only() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let first = test_version();
    publish(&fixture, &owner, &org, "deployment", &first).await;
    let outsider = register_user(&fixture).await;

    let refused = resolve_as_consumer(&fixture, &outsider.token, &org, "deployment", &first)
        .await
        .expect_err("private until the organisation is made public");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    set_org_visibility(&fixture, &owner.token, &org, "public")
        .await
        .expect("an org admin makes the organisation public");

    resolve_as_consumer(&fixture, &outsider.token, &org, "deployment", &first)
        .await
        .expect("an existing component of a public organisation is readable");
    let later = test_version();
    publish(&fixture, &owner, &org, "sdk", &later).await;
    resolve_as_consumer(&fixture, &outsider.token, &org, "sdk", &later)
        .await
        .expect("a component published later is readable too");
    fixture
        .registry()
        .list_org_tools(authed_request(
            &outsider.token,
            ListOrgToolsRequest {
                organisation: org.clone(),
            },
        ))
        .await
        .expect("a public organisation's tool listing is readable");

    let refused = fixture
        .registry()
        .begin_upload(authed_request(
            &outsider.token,
            BeginUploadRequest {
                name: "deployment".into(),
                organisation: org.clone(),
                version: test_version(),
            },
        ))
        .await
        .expect_err("a public organisation is still read-only to outsiders");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let refused = fixture
        .releases()
        .get_projects(authed_request(
            &outsider.token,
            GetProjectsRequest {
                query: Some(get_projects_request::Query::Organisation(OrganisationRef {
                    organisation: org.clone(),
                })),
            },
        ))
        .await
        .expect_err("only components are shared, not the organisation's projects");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let refused = set_org_visibility(&fixture, &outsider.token, &org, "private")
        .await
        .expect_err("a non-member cannot change the organisation's visibility");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    set_org_visibility(&fixture, &owner.token, &org, "private")
        .await
        .unwrap();
    let refused = resolve_as_consumer(&fixture, &outsider.token, &org, "deployment", &first)
        .await
        .expect_err("private again");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_org_admin_changes_organisation_visibility() {
    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let member = register_user(&fixture).await;
    add_member(&fixture, &org, &member).await;

    let refused = set_org_visibility(&fixture, &member.token, &org, "public")
        .await
        .expect_err("a plain member cannot open the organisation");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let refused = set_org_visibility(&fixture, &owner.token, &org, "internal")
        .await
        .expect_err("only public and private exist");
    assert_eq!(refused.code(), tonic::Code::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_token_cannot_change_visibility() {
    use forest_grpc_interface::app_service_client::AppServiceClient;

    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    publish(&fixture, &owner, &org, "widget", &test_version()).await;
    let member = register_user(&fixture).await;
    add_member(&fixture, &org, &member).await;

    let org_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM organisations WHERE name = $1")
        .bind(&org)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    let mut apps = AppServiceClient::new(fixture.channel.clone());
    let app = apps
        .create_app(authed_request(
            &member.token,
            CreateAppRequest {
                organisation_id: org_id.to_string(),
                name: format!("ci-{}", uuid::Uuid::now_v7().simple()),
                description: String::new(),
                permissions: vec![],
            },
        ))
        .await
        .expect("a plain member can create an app")
        .into_inner()
        .app
        .expect("app");
    let app_token = apps
        .create_app_token(authed_request(
            &member.token,
            CreateAppTokenRequest {
                app_id: app.app_id,
                name: "ci".into(),
                expires_in_seconds: 0,
            },
        ))
        .await
        .expect("and mint a token for it")
        .into_inner()
        .raw_token;

    let refused = set_visibility(&fixture, &app_token, &org, "widget", "public")
        .await
        .expect_err("an app token must not open a component");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    let refused = set_org_visibility(&fixture, &app_token, &org, "public")
        .await
        .expect_err("an app token must not open the organisation");
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    fixture
        .releases()
        .update_project(authed_request(
            &app_token,
            UpdateProjectRequest {
                organisation: org.clone(),
                project: "widget".into(),
                readme: None,
                description: Some("from ci".into()),
                metadata: None,
                visibility: None,
            },
        ))
        .await
        .expect("an app token still updates project details, as publish does");
}

async fn org_visibility(fixture: &Fixture, organisation: &str) -> String {
    sqlx::query_scalar("SELECT visibility FROM organisations WHERE name = $1")
        .bind(organisation)
        .fetch_one(&fixture.db)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_organisation_visibility_changes_leave_it_unchanged() {
    use forest_grpc_interface::app_service_client::AppServiceClient;

    let fixture = fixture().await.unwrap();
    let owner = register_user(&fixture).await;
    let org = create_org(&fixture, &owner).await;
    let member = register_user(&fixture).await;
    add_member(&fixture, &org, &member).await;
    let outsider = register_user(&fixture).await;

    let org_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM organisations WHERE name = $1")
        .bind(&org)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    let mut apps = AppServiceClient::new(fixture.channel.clone());
    let app_id = apps
        .create_app(authed_request(
            &owner.token,
            CreateAppRequest {
                organisation_id: org_id.to_string(),
                name: format!("ci-{}", uuid::Uuid::now_v7().simple()),
                description: String::new(),
                permissions: vec![],
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .app
        .unwrap()
        .app_id;
    let app_token = apps
        .create_app_token(authed_request(
            &owner.token,
            CreateAppTokenRequest {
                app_id,
                name: "ci".into(),
                expires_in_seconds: 0,
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .raw_token;

    for token in [&member.token, &outsider.token, &app_token] {
        assert!(
            set_org_visibility(&fixture, token, &org, "public")
                .await
                .is_err()
        );
        assert_eq!(org_visibility(&fixture, &org).await, "private");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn search_shows_a_public_organisations_components_and_hides_other_private_ones() {
    let fixture = fixture().await.unwrap();
    let public_owner = register_user(&fixture).await;
    let public_org = create_org(&fixture, &public_owner).await;
    publish(
        &fixture,
        &public_owner,
        &public_org,
        "shared",
        &test_version(),
    )
    .await;
    set_org_visibility(&fixture, &public_owner.token, &public_org, "public")
        .await
        .unwrap();

    let private_owner = register_user(&fixture).await;
    let private_org = create_org(&fixture, &private_owner).await;
    publish(
        &fixture,
        &private_owner,
        &private_org,
        "shared",
        &test_version(),
    )
    .await;

    let outsider = register_user(&fixture).await;
    let mut registry = fixture.registry();

    for org in [&public_org, &private_org] {
        let expected = org == &public_org;

        let found = registry
            .search_components(authed_request(
                &outsider.token,
                SearchComponentsRequest {
                    query: String::new(),
                    organisation: org.clone(),
                    page: 0,
                    page_size: 100,
                },
            ))
            .await
            .unwrap()
            .into_inner()
            .components;
        assert_eq!(
            found.iter().any(|c| &c.organisation == org),
            expected,
            "signed-in search for {org}"
        );

        let found = registry
            .search_public_components(tonic::Request::new(SearchPublicComponentsRequest {
                query: String::new(),
                organisation: org.clone(),
                page: 0,
                page_size: 100,
            }))
            .await
            .unwrap()
            .into_inner()
            .components;
        assert_eq!(
            found.iter().any(|c| &c.organisation == org),
            expected,
            "anonymous search for {org}"
        );

        let detail = registry
            .get_public_component_detail(tonic::Request::new(GetPublicComponentDetailRequest {
                organisation: org.clone(),
                name: "shared".into(),
            }))
            .await;
        assert_eq!(detail.is_ok(), expected, "anonymous detail for {org}");
    }
}
