use forest_grpc_interface::*;
use tonic::metadata::MetadataValue;

use crate::accepttest::release_flow::ReleaseFlowData;

use super::Then;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: MetadataValue<_> = format!("Bearer {}", token).parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

pub trait ThenReleaseFlow {
    async fn release_is_in_terminal_state(self) -> anyhow::Result<Then<ReleaseFlowData>>;
    async fn artifact_is_retrievable_by_slug(self) -> anyhow::Result<Then<ReleaseFlowData>>;
    async fn artifact_is_listed_in_project(self) -> anyhow::Result<Then<ReleaseFlowData>>;
}

impl ThenReleaseFlow for Then<ReleaseFlowData> {
    async fn release_is_in_terminal_state(self) -> anyhow::Result<Self> {
        let status = self.data().terminal_status.clone();
        assert!(
            matches!(
                status.as_str(),
                "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT"
            ),
            "release should be in terminal state, got: {}",
            status
        );
        Ok(self)
    }

    async fn artifact_is_retrievable_by_slug(self) -> anyhow::Result<Self> {
        let mut release_client = self.fixture().releases();
        let (token, slug, artifact_id) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.slug.clone(),
                data.artifact_id.clone(),
            )
        };

        let resp = release_client
            .get_artifact_by_slug(authed_request(
                &token,
                GetArtifactBySlugRequest { slug: slug.clone() },
            ))
            .await?;

        let artifact = resp.into_inner().artifact.expect("artifact in response");
        assert_eq!(artifact.slug, slug);
        assert_eq!(artifact.artifact_id, artifact_id);

        Ok(self)
    }

    async fn artifact_is_listed_in_project(self) -> anyhow::Result<Self> {
        let mut release_client = self.fixture().releases();
        let (token, slug, dest_name, org) = {
            let data = self.data();
            (
                data.auth_token.clone(),
                data.slug.clone(),
                data.destination_name.clone(),
                data.organisation.clone(),
            )
        };

        let resp = release_client
            .get_artifacts_by_project(authed_request(
                &token,
                GetArtifactsByProjectRequest {
                    project: Some(Project {
                        organisation: org,
                        project: "test-project".into(),
                    }),
                },
            ))
            .await?;

        let artifacts = resp.into_inner().artifact;
        assert!(!artifacts.is_empty(), "should have at least one artifact");

        let found = artifacts.iter().find(|a| a.slug == slug);
        assert!(found.is_some(), "should find our artifact in the list");

        let art = found.unwrap();
        assert!(
            !art.destinations.is_empty(),
            "artifact should have destinations after release"
        );
        assert_eq!(art.destinations[0].name, dest_name);

        Ok(self)
    }
}
