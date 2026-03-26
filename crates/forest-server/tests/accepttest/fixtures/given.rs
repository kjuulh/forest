use std::collections::HashMap;

use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::release_flow::ReleaseFlowData;

use super::Given;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

pub trait GivenReleaseFlow {
    async fn a_registered_user(self) -> Self;
    async fn an_organisation(self, name: &str) -> Self;
    async fn an_environment(self, name: &str) -> Self;
    async fn a_destination(self, name: &str, environment: &str) -> Self;
    async fn an_uploaded_artifact(self) -> Self;
    async fn an_annotated_release(self) -> Self;
}

impl GivenReleaseFlow for Given<ReleaseFlowData> {
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

    async fn an_organisation(self, name: &str) -> Self {
        let mut org_client = self.fixture().organisations();
        let token = self.data().auth_token.clone();

        org_client
            .create_organisation(authed_request(
                &token,
                CreateOrganisationRequest {
                    name: name.into(),
                },
            ))
            .await
            .expect("create organisation");

        self.data_mut().organisation = name.into();

        self
    }

    async fn an_environment(self, name: &str) -> Self {
        let mut env_client = self.fixture().environments();

        let (token, org) = {
            let data = self.data();
            (data.auth_token.clone(), data.organisation.clone())
        };

        env_client
            .create_environment(authed_request(
                &token,
                CreateEnvironmentRequest {
                    organisation: org,
                    name: name.into(),
                    description: None,
                    sort_order: 0,
                },
            ))
            .await
            .expect("create environment");

        self
    }

    async fn a_destination(self, name: &str, environment: &str) -> Self {
        let mut dest_client = self.fixture().destinations();

        // Create a temp directory for the local flux destination
        let local_path = format!("/tmp/forest-accept-test-{}", uuid::Uuid::now_v7());
        std::fs::create_dir_all(&local_path).expect("create local path");

        let mut metadata = HashMap::new();
        metadata.insert("cluster_name".into(), "test-cluster".into());
        metadata.insert("namespace".into(), "test-namespace".into());
        metadata.insert("local_path".into(), local_path.clone());

        let (token, org) = {
            let data = self.data();
            (data.auth_token.clone(), data.organisation.clone())
        };

        dest_client
            .create_destination(authed_request(
                &token,
                CreateDestinationRequest {
                    organisation: org,
                    name: name.into(),
                    environment: environment.into(),
                    metadata,
                    r#type: Some(DestinationType {
                        organisation: "forest".into(),
                        name: "flux".into(),
                        version: 1,
                    }),
                },
            ))
            .await
            .expect("create destination");

        {
            let mut data = self.data_mut();
            data.destination_name = name.into();
            data.destination_environment = environment.into();
            data.local_path = local_path;
        }

        self
    }

    async fn an_uploaded_artifact(self) -> Self {
        let mut art_client = self.fixture().artifacts();
        let (token, dest, env) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.destination_name.clone(),
                data.destination_environment.clone(),
            )
        };

        // Begin
        let begin_resp = art_client
            .begin_upload_artifact(authed_request(&token, BeginUploadArtifactRequest {}))
            .await
            .expect("begin upload");
        let upload_id = begin_resp.into_inner().upload_id;

        // Upload a deployment file
        let upload_stream = tokio_stream::iter(vec![UploadArtifactRequest {
            upload_id: upload_id.clone(),
            file_name: "deployment.yaml".into(),
            file_content: "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: test-app\n"
                .into(),
            env,
            destination: dest,
            category: "deployment".into(),
        }]);

        let mut req = tonic::Request::new(upload_stream);
        let val: MetadataValue<_> = format!("Bearer {}", token).parse().unwrap();
        req.metadata_mut().insert("authorization", val.clone());
        art_client
            .upload_artifact(req)
            .await
            .expect("upload artifact");

        // Commit
        let commit_resp = art_client
            .commit_artifact(authed_request(&token, CommitArtifactRequest { upload_id }))
            .await
            .expect("commit artifact");

        self.data_mut().artifact_id = commit_resp.into_inner().artifact_id;

        self
    }

    async fn an_annotated_release(self) -> Self {
        let mut release_client = self.fixture().releases();
        let (token, artifact_id, org) = {
            let data = self.data();
            (data.auth_token.clone(), data.artifact_id.clone(), data.organisation.clone())
        };

        let resp = release_client
            .annotate_release(authed_request(
                &token,
                AnnotateReleaseRequest {
                    artifact_id,
                    project: Some(Project {
                        organisation: org,
                        project: "test-project".into(),
                    }),
                    metadata: HashMap::new(),
                    source: Some(Source {
                        user: Some("test-user".into()),
                        email: Some("test@example.com".into()),
                        user_id: None,
                        source_type: Some("ci".into()),
                        run_url: Some("https://ci.example.com/run/123".into()),
                    }),
                    context: Some(ArtifactContext {
                        title: "Test release".into(),
                        description: Some("Acceptance test release".into()),
                        web: None,
                        pr: None,
                    }),
                    r#ref: Some(Ref {
                        commit_sha: "abc123def456".into(),
                        branch: Some("main".into()),
                        commit_message: Some("test commit".into()),
                        version: Some("1.0.0".into()),
                        repo_url: Some("https://example.com/repo".into()),
                    }),
                    annotation_only: false,
                },
            ))
            .await
            .expect("annotate release");

        let artifact = resp.into_inner().artifact.expect("artifact");
        self.data_mut().slug = artifact.slug;

        self
    }
}
