use forest_grpc_interface::*;
use futures::StreamExt;
use tonic::metadata::MetadataValue;

use crate::accepttest::fixtures::{fixture, testcase, Given, Then, When};

// ============================================================
// Test data
// ============================================================

#[derive(Clone, Default)]
pub struct ComponentFlowData {
    pub auth_token: String,
    pub upload_context: String,
    pub component_name: String,
    pub component_org: String,
    pub component_version: String,
}

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

// ============================================================
// Given
// ============================================================

trait GivenComponentFlow {
    async fn a_registered_user(self) -> Self;
    async fn a_component(self, org: &str, name: &str, version: &str) -> Self;
    async fn a_begun_upload(self) -> Self;
    async fn files_uploaded(self, files: &[(&str, &[u8])]) -> Self;
    async fn upload_committed(self) -> Self;
}

impl GivenComponentFlow for Given<ComponentFlowData> {
    async fn a_registered_user(self) -> Self {
        let mut users = self.fixture().users();

        let username = format!("testuser-{}", uuid::Uuid::now_v7());
        let email = format!("test-{}@example.com", uuid::Uuid::now_v7());

        let resp = users
            .register(RegisterRequest {
                username,
                email,
                password: "TestPassword123!".into(),
            })
            .await
            .expect("register user");

        let tokens = resp.into_inner().tokens.expect("tokens");
        self.data_mut().auth_token = tokens.access_token;

        self
    }

    async fn a_component(self, org: &str, name: &str, version: &str) -> Self {
        {
            let mut data = self.data_mut();
            data.component_org = org.into();
            data.component_name = name.into();
            data.component_version = version.into();
        }
        self
    }

    async fn a_begun_upload(self) -> Self {
        let (token, org, name, version) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
                data.component_version.clone(),
            )
        };

        let resp = self
            .fixture()
            .registry()
            .begin_upload(authed_request(
                &token,
                BeginUploadRequest {
                    name,
                    organisation: org,
                    version,
                },
            ))
            .await
            .expect("begin upload");

        self.data_mut().upload_context = resp.into_inner().upload_context;
        self
    }

    async fn files_uploaded(self, files: &[(&str, &[u8])]) -> Self {
        let (token, ctx) = {
            let data = self.data();
            (data.auth_token.clone(), data.upload_context.clone())
        };
        let mut client = self.fixture().registry();

        for (path, content) in files {
            client
                .upload_file(authed_request(
                    &token,
                    UploadFileRequest {
                        upload_context: ctx.clone(),
                        file_path: path.to_string(),
                        file_content: content.to_vec(),
                    },
                ))
                .await
                .expect("upload file");
        }

        self
    }

    async fn upload_committed(self) -> Self {
        let (token, ctx) = {
            let data = self.data();
            (data.auth_token.clone(), data.upload_context.clone())
        };

        self.fixture()
            .registry()
            .commit_upload(authed_request(
                &token,
                CommitUploadRequest {
                    upload_context: ctx,
                },
            ))
            .await
            .expect("commit upload");

        self
    }
}

// ============================================================
// When
// ============================================================

trait WhenComponentFlow {
    async fn begin_upload_is_called(self) -> Self;
    async fn files_are_uploaded(self, files: &[(&str, &[u8])]) -> Self;
    async fn upload_is_committed(self) -> Self;
}

impl WhenComponentFlow for When<ComponentFlowData> {
    async fn begin_upload_is_called(self) -> Self {
        let (token, org, name, version) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
                data.component_version.clone(),
            )
        };

        let resp = self
            .fixture()
            .registry()
            .begin_upload(authed_request(
                &token,
                BeginUploadRequest {
                    name,
                    organisation: org,
                    version,
                },
            ))
            .await
            .expect("begin upload");

        self.data_mut().upload_context = resp.into_inner().upload_context;
        self
    }

    async fn files_are_uploaded(self, files: &[(&str, &[u8])]) -> Self {
        let (token, ctx) = {
            let data = self.data();
            (data.auth_token.clone(), data.upload_context.clone())
        };
        let mut client = self.fixture().registry();

        for (path, content) in files {
            client
                .upload_file(authed_request(
                    &token,
                    UploadFileRequest {
                        upload_context: ctx.clone(),
                        file_path: path.to_string(),
                        file_content: content.to_vec(),
                    },
                ))
                .await
                .expect("upload file");
        }

        self
    }

    async fn upload_is_committed(self) -> Self {
        let (token, ctx) = {
            let data = self.data();
            (data.auth_token.clone(), data.upload_context.clone())
        };

        self.fixture()
            .registry()
            .commit_upload(authed_request(
                &token,
                CommitUploadRequest {
                    upload_context: ctx,
                },
            ))
            .await
            .expect("commit upload");

        self
    }
}

// ============================================================
// Then
// ============================================================

trait ThenComponentFlow {
    async fn component_is_queryable(self) -> Self;
    async fn component_version_is_queryable(self) -> Self;
    async fn component_files_are_retrievable(self, expected: &[(&str, &[u8])]) -> Self;
    async fn component_is_not_found(self) -> Self;
}

impl ThenComponentFlow for Then<ComponentFlowData> {
    async fn component_is_queryable(self) -> Self {
        let (token, org, name) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
            )
        };

        let resp = self
            .fixture()
            .registry()
            .get_component(authed_request(
                &token,
                GetComponentRequest {
                    name,
                    organisation: org,
                },
            ))
            .await
            .expect("get component");

        let component = resp.into_inner().component.expect("component should exist");
        let expected_version = self.data().component_version.clone();
        assert_eq!(component.version, expected_version);

        self
    }

    async fn component_version_is_queryable(self) -> Self {
        let (token, org, name, version) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
                data.component_version.clone(),
            )
        };

        let resp = self
            .fixture()
            .registry()
            .get_component_version(authed_request(
                &token,
                GetComponentVersionRequest {
                    name,
                    organisation: org,
                    version: version.clone(),
                },
            ))
            .await
            .expect("get component version");

        let component = resp
            .into_inner()
            .component
            .expect("component version should exist");
        assert_eq!(component.version, version);

        self
    }

    async fn component_files_are_retrievable(self, expected: &[(&str, &[u8])]) -> Self {
        let (token, org, name, version) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
                data.component_version.clone(),
            )
        };

        // Get the component id
        let resp = self
            .fixture()
            .registry()
            .get_component_version(authed_request(
                &token,
                GetComponentVersionRequest {
                    name,
                    organisation: org,
                    version,
                },
            ))
            .await
            .expect("get component version for files");

        let component_id = resp.into_inner().component.expect("component").id;

        // Stream files
        let resp = self
            .fixture()
            .registry()
            .get_component_files(authed_request(
                &token,
                GetComponentFilesRequest { component_id },
            ))
            .await
            .expect("get component files");

        let mut stream = resp.into_inner();
        let mut received_files: Vec<(String, Vec<u8>)> = Vec::new();

        while let Some(msg) = stream.next().await {
            let msg = msg.expect("stream message");
            match msg.msg {
                Some(get_component_files_response::Msg::ComponentFile(f)) => {
                    received_files.push((f.file_path, f.file_content));
                }
                Some(get_component_files_response::Msg::Done(_)) => break,
                None => panic!("unexpected empty message"),
            }
        }

        assert_eq!(
            received_files.len(),
            expected.len(),
            "file count mismatch: got {:?}",
            received_files.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );

        for (expected_path, expected_content) in expected {
            let found = received_files
                .iter()
                .find(|(p, _)| p == expected_path)
                .unwrap_or_else(|| panic!("expected file {} not found", expected_path));
            assert_eq!(
                &found.1, expected_content,
                "content mismatch for {}",
                expected_path
            );
        }

        self
    }

    async fn component_is_not_found(self) -> Self {
        let (token, org, name) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.component_org.clone(),
                data.component_name.clone(),
            )
        };

        let resp = self
            .fixture()
            .registry()
            .get_component(authed_request(
                &token,
                GetComponentRequest {
                    name,
                    organisation: org,
                },
            ))
            .await
            .expect("get component");

        assert!(
            resp.into_inner().component.is_none(),
            "component should not exist yet"
        );

        self
    }
}

// ============================================================
// Test scenarios
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_full_component_upload_flow() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ComponentFlowData>().await?;

    let org = format!("comp-org-{}", uuid::Uuid::now_v7());
    let name = format!("comp-{}", uuid::Uuid::now_v7());

    // Given a registered user and component definition
    given
        .a_registered_user()
        .await
        .a_component(&org, &name, "1.0.0")
        .await;

    // When we go through the full upload flow
    when.begin_upload_is_called()
        .await
        .files_are_uploaded(&[
            ("deployment.yaml", b"apiVersion: apps/v1\nkind: Deployment"),
            ("service.yaml", b"apiVersion: v1\nkind: Service"),
        ])
        .await
        .upload_is_committed()
        .await;

    // Then the component and its files are queryable
    then.component_is_queryable()
        .await
        .component_version_is_queryable()
        .await
        .component_files_are_retrievable(&[
            ("deployment.yaml", b"apiVersion: apps/v1\nkind: Deployment"),
            ("service.yaml", b"apiVersion: v1\nkind: Service"),
        ])
        .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_component_not_found_before_commit() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ComponentFlowData>().await?;

    let org = format!("comp-org-{}", uuid::Uuid::now_v7());
    let name = format!("comp-{}", uuid::Uuid::now_v7());

    given
        .a_registered_user()
        .await
        .a_component(&org, &name, "1.0.0")
        .await;

    // Begin upload but don't commit
    when.begin_upload_is_called()
        .await
        .files_are_uploaded(&[("file.yaml", b"content")])
        .await;

    // Component should not be queryable yet
    then.component_is_not_found().await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_superseded_upload_replaces_inflight() -> anyhow::Result<()> {
    let (given, _when, then) = testcase::<ComponentFlowData>().await?;

    let org = format!("comp-org-{}", uuid::Uuid::now_v7());
    let name = format!("comp-{}", uuid::Uuid::now_v7());

    given
        .clone()
        .a_registered_user()
        .await
        .a_component(&org, &name, "1.0.0")
        .await;

    // Begin first upload, upload a file, but don't commit
    given
        .clone()
        .a_begun_upload()
        .await
        .files_uploaded(&[("old.yaml", b"old content")])
        .await;

    // Begin second upload for same version — supersedes the first
    // Then upload new files and commit
    given
        .a_begun_upload()
        .await
        .files_uploaded(&[("new.yaml", b"new content")])
        .await
        .upload_committed()
        .await;

    // Only the second upload's files should be present
    then.component_is_queryable()
        .await
        .component_files_are_retrievable(&[("new.yaml", b"new content")])
        .await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_versions_of_same_component() -> anyhow::Result<()> {
    let (given, _when, then) = testcase::<ComponentFlowData>().await?;

    let org = format!("comp-org-{}", uuid::Uuid::now_v7());
    let name = format!("comp-{}", uuid::Uuid::now_v7());

    // Register user first
    given
        .clone()
        .a_registered_user()
        .await
        .a_component(&org, &name, "1.0.0")
        .await;

    // Publish v1.0.0
    given
        .clone()
        .a_begun_upload()
        .await
        .files_uploaded(&[("v1.yaml", b"v1")])
        .await
        .upload_committed()
        .await;

    // Publish v2.0.0
    given
        .clone()
        .a_component(&org, &name, "2.0.0")
        .await
        .a_begun_upload()
        .await
        .files_uploaded(&[("v2.yaml", b"v2")])
        .await
        .upload_committed()
        .await;

    // Latest should be v2.0.0
    then.clone().component_is_queryable().await;

    // v1.0.0 should still be queryable by version
    given.a_component(&org, &name, "1.0.0").await;
    then.component_version_is_queryable().await;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cannot_republish_same_version() -> anyhow::Result<()> {
    let fixture = fixture().await?;

    // Register a user first
    let mut users = forest_grpc_interface::users_service_client::UsersServiceClient::new(
        fixture.channel.clone(),
    );
    let resp = users
        .register(RegisterRequest {
            username: format!("testuser-{}", uuid::Uuid::now_v7()),
            email: format!("test-{}@example.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");
    let token = resp.into_inner().tokens.expect("tokens").access_token;

    let org = format!("comp-org-{}", uuid::Uuid::now_v7());
    let name = format!("comp-{}", uuid::Uuid::now_v7());
    let version = "1.0.0";

    let mut client = fixture.registry();

    // Publish v1.0.0
    let resp = client
        .begin_upload(authed_request(
            &token,
            BeginUploadRequest {
                name: name.clone(),
                organisation: org.clone(),
                version: version.into(),
            },
        ))
        .await
        .expect("begin upload");
    let ctx = resp.into_inner().upload_context;

    client
        .upload_file(authed_request(
            &token,
            UploadFileRequest {
                upload_context: ctx.clone(),
                file_path: "app.yaml".into(),
                file_content: b"content".to_vec(),
            },
        ))
        .await
        .expect("upload file");

    client
        .commit_upload(authed_request(
            &token,
            CommitUploadRequest {
                upload_context: ctx,
            },
        ))
        .await
        .expect("commit");

    // Try to publish v1.0.0 again — should fail
    let result = client
        .begin_upload(authed_request(
            &token,
            BeginUploadRequest {
                name: name.clone(),
                organisation: org.clone(),
                version: version.into(),
            },
        ))
        .await;

    assert!(
        result.is_err(),
        "should reject duplicate version upload: {:?}",
        result
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_upload_file_with_invalid_context() -> anyhow::Result<()> {
    let fixture = fixture().await?;

    // Register user
    let mut users = forest_grpc_interface::users_service_client::UsersServiceClient::new(
        fixture.channel.clone(),
    );
    let resp = users
        .register(RegisterRequest {
            username: format!("testuser-{}", uuid::Uuid::now_v7()),
            email: format!("test-{}@example.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");
    let token = resp.into_inner().tokens.expect("tokens").access_token;

    let mut client = fixture.registry();

    // Try uploading with a completely bogus upload context
    let result = client
        .upload_file(authed_request(
            &token,
            UploadFileRequest {
                upload_context: uuid::Uuid::now_v7().to_string(),
                file_path: "file.yaml".into(),
                file_content: b"content".to_vec(),
            },
        ))
        .await;

    assert!(result.is_err(), "should reject unknown upload context");

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_commit_with_invalid_context() -> anyhow::Result<()> {
    let fixture = fixture().await?;

    // Register user
    let mut users = forest_grpc_interface::users_service_client::UsersServiceClient::new(
        fixture.channel.clone(),
    );
    let resp = users
        .register(RegisterRequest {
            username: format!("testuser-{}", uuid::Uuid::now_v7()),
            email: format!("test-{}@example.com", uuid::Uuid::now_v7()),
            password: "TestPassword123!".into(),
        })
        .await
        .expect("register user");
    let token = resp.into_inner().tokens.expect("tokens").access_token;

    let mut client = fixture.registry();

    let result = client
        .commit_upload(authed_request(
            &token,
            CommitUploadRequest {
                upload_context: uuid::Uuid::now_v7().to_string(),
            },
        ))
        .await;

    assert!(result.is_err(), "should reject unknown upload context");

    Ok(())
}
