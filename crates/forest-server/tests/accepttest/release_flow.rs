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
    let org = format!("test-org-{}", uuid::Uuid::now_v7());
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .a_destination("accept-dest", "accept-env")
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

#[tokio::test(flavor = "multi_thread")]
async fn test_annotate_and_retrieve_artifact() -> anyhow::Result<()> {
    let (given, _when, then) = testcase::<ReleaseFlowData>().await?;

    let org = format!("test-org-{}", uuid::Uuid::now_v7());
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .a_destination("retrieve-dest", "retrieve-env")
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    then.artifact_is_retrievable_by_slug().await?;

    Ok(())
}
