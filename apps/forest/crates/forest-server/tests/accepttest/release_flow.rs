use crate::accepttest::fixtures::{GivenReleaseFlow, ThenReleaseFlow, WhenReleaseFlow, testcase};

#[derive(Clone, Default)]
pub struct ReleaseFlowData {
    pub auth_token: String,
    pub organisation: String,
    pub local_path: String,
    pub destination_name: String,
    pub destination_environment: String,
    pub artifact_id: String,
    pub slug: String,
    pub release_intent_id: String,
    pub terminal_status: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn test_full_release_flow() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ReleaseFlowData>().await?;

    // Given
    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let dest = format!("accept-dest-{suffix}");
    let env = format!("accept-env-{suffix}");
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&dest, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    // When
    when.release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    // Then
    then.release_is_in_terminal_state()
        .await?
        .artifact_is_retrievable_by_slug()
        .await?
        .artifact_is_listed_in_project()
        .await?;

    Ok(())
}

/// The announce-only failure path (DATA-637): a project whose rollout Forest
/// does not perform annotates up front, deploys elsewhere, and reports back that
/// it failed. That has to leave the same kind of record a success would — an
/// intent, a release, and a finalized status — or the project's failures are
/// invisible to everything that reads releases.
#[tokio::test(flavor = "multi_thread")]
async fn test_externally_failed_release_is_recorded_and_finalized() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let dest = format!("accept-dest-{suffix}");
    let env = format!("accept-env-{suffix}");
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&dest, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    when.release_is_reported_failed("ECS rollout did not converge")
        .await?;

    then.a_failed_release_is_recorded()
        .await?
        .the_intent_is_finalized_failed()
        .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_annotate_and_retrieve_artifact() -> anyhow::Result<()> {
    let (given, _when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let dest = format!("retrieve-dest-{suffix}");
    let env = format!("retrieve-env-{suffix}");
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&dest, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    then.artifact_is_retrievable_by_slug().await?;

    Ok(())
}
