use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;
use forest_grpc_interface::ReleaseRequest;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

#[tokio::test(flavor = "multi_thread")]
async fn naming_a_destination_does_not_release_to_its_neighbours() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let target = format!("accept-dest-target-{suffix}");
    let neighbour = format!("accept-dest-neighbour-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&target, &env)
        .await
        .a_destination(&neighbour, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let resp = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![target.clone()],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();

    let released: Vec<String> = resp
        .intents
        .iter()
        .map(|intent| intent.destination.clone())
        .collect();

    assert_eq!(
        released,
        vec![target.clone()],
        "naming {target} should release to {target} alone; the environment must not \
         pull in {neighbour}",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_alone_still_releases_to_everything_in_it() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let first = format!("accept-dest-a-{suffix}");
    let second = format!("accept-dest-b-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&first, &env)
        .await
        .a_destination(&second, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let resp = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();

    let mut released: Vec<String> = resp
        .intents
        .iter()
        .map(|intent| intent.destination.clone())
        .collect();
    released.sort();

    let mut expected = vec![first, second];
    expected.sort();

    assert_eq!(
        released, expected,
        "an environment with no named destinations should still reach all of them",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_misspelled_destination_is_rejected_not_expanded() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let real = format!("accept-dest-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&real, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let _err = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec!["accept-dest-typo".into()],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await
        .expect_err(
            "a destination that does not exist must be refused, not silently widened to \
             every destination in the environment",
        );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_is_narrowed_by_what_the_project_declared() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let target = format!("target-{suffix}");
    let neighbour = format!("neighbour-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&target, &env)
        .await
        .a_destination(&neighbour, &env)
        .await
        .an_uploaded_artifact_declaring(&env, &["^target-.*$"])
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let resp = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();

    let released: Vec<String> = resp
        .intents
        .iter()
        .map(|intent| intent.destination.clone())
        .collect();

    assert_eq!(
        released,
        vec![target.clone()],
        "declaring ^target-.*$ should release to {target} alone; {neighbour} shares the \
         environment but the project did not ask for it",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn naming_a_destination_the_project_does_not_declare_still_releases_there()
-> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let declared = format!("target-{suffix}");
    let other = format!("neighbour-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&declared, &env)
        .await
        .a_destination(&other, &env)
        .await
        .an_uploaded_artifact_declaring(&env, &["^target-.*$"])
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let resp = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![other.clone()],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();

    let released: Vec<String> = resp
        .intents
        .iter()
        .map(|intent| intent.destination.clone())
        .collect();

    assert_eq!(
        released,
        vec![other],
        "an explicitly named destination must still be released to, declaration or not",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_matching_nothing_refuses_the_request() -> anyhow::Result<()> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&format!("dest-{suffix}"), &env)
        .await
        .an_uploaded_artifact_declaring(&env, &["^nowhere/.*$"])
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    when.fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![],
                environments: vec![env.clone()],
                force: false,
                use_pipeline: false,
                prepare_only: false,
            },
        ))
        .await
        .expect_err("a declaration matching nothing must be refused, not released nowhere");

    Ok(())
}
