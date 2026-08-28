//! Naming a destination releases to that destination.
//!
//! This used to be an `OR`: `resolve_destinations` matched
//! `d.name = ANY($destinations) OR e.name = ANY($environments)`, so a request
//! carrying both got the union. That mattered because the CLI resolves an
//! environment even when `--destination` is given, so naming one destination in
//! a shared environment quietly released to every destination in it.
//!
//! Found the hard way: a release scoped to a single ECS destination also went to
//! the terraform destination sharing its environment. It failed harmlessly there
//! only because the artifact had no manifests to plan.

use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;
use forest_grpc_interface::ReleaseRequest;

/// Local copy: the identical helper is module-private in each flow module, and
/// they each keep their own rather than widening it.
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

    // Two destinations, one environment — the shape that exposed the bug. A
    // single destination in an environment cannot tell the two behaviours apart.
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

    // Send both, exactly as the CLI did: one named destination *and* the
    // environment it happens to live in.
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

    // The other half of the contract: fan-out is still what you get when you ask
    // for an environment and name nothing. Narrowing `--destination` must not
    // have narrowed this too.
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
    // A real destination in the environment, so the typo has something to have
    // been wrongly expanded *to*.
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

    // The safety property, not the wording: before the fix a typo'd destination
    // combined with an environment still matched via the `OR`, so the release
    // quietly went to the whole environment instead of failing. It must now be
    // refused outright, with nothing released.
    //
    // Only that it fails is asserted. `to_internal_error` maps anyhow through
    // `e.to_string()`, which keeps just the outermost context, so the client
    // sees "release" while the server log carries the chain — including the
    // name of the destination that was not found.
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
