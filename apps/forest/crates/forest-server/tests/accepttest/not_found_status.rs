use forest_grpc_interface::{
    GetArtifactBySlugRequest, ListTriggersRequest, Project, trigger_service_client,
};
use uuid::Uuid;

use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

fn authed<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid metadata"),
    );
    req
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_release_slug_is_not_found() -> anyhow::Result<()> {
    let (given, _, then) = testcase::<ReleaseFlowData>().await?;
    let given = given.a_registered_user().await;
    let token = given.data().auth_token.clone();

    let status = then
        .fixture()
        .releases()
        .get_artifact_by_slug(authed(
            &token,
            GetArtifactBySlugRequest {
                slug: "list".into(),
            },
        ))
        .await
        .expect_err("no release has the slug 'list'");

    assert_eq!(status.code(), tonic::Code::NotFound, "{status:?}");
    assert!(status.message().contains("list"), "{status:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_project_is_not_found() -> anyhow::Result<()> {
    let (given, _, then) = testcase::<ReleaseFlowData>().await?;
    let org = format!("test-org-{}", Uuid::now_v7());
    let given = given.a_registered_user().await.an_organisation(&org).await;
    let token = given.data().auth_token.clone();

    let mut triggers =
        trigger_service_client::TriggerServiceClient::new(then.fixture().channel.clone());
    let status = triggers
        .list_triggers(authed(
            &token,
            ListTriggersRequest {
                project: Some(Project {
                    organisation: org.clone(),
                    project: "no-such-project".into(),
                    ..Default::default()
                }),
            },
        ))
        .await
        .expect_err("the project does not exist");

    assert_eq!(status.code(), tonic::Code::NotFound, "{status:?}");
    assert!(status.message().contains("no-such-project"), "{status:?}");
    Ok(())
}
