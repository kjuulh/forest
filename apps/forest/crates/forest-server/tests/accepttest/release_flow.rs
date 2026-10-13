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

/// Two artifacts, two destinations, one project — the shape the project
/// listing's destination query is grouped by.
///
/// `test_full_release_flow` releases one artifact, so it passes whatever the
/// listing does with the rows it reads back. This one releases two to different
/// destinations, which is the smallest case where attributing a destination to
/// the wrong artifact is visible at all.
#[tokio::test(flavor = "multi_thread")]
async fn test_project_listing_keeps_each_artifacts_destinations_apart() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let dest_a = format!("accept-dest-a-{suffix}");
    let dest_b = format!("accept-dest-b-{suffix}");

    given
        .clone()
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&dest_a, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    when.clone()
        .release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    // The fixture keeps one slug at a time, so the first release's has to be
    // taken before the second overwrites it.
    let slug_a = then.data().slug.clone();

    given
        .a_destination(&dest_b, &env)
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    when.release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    let slug_b = then.data().slug.clone();
    assert_ne!(slug_a, slug_b, "the two releases should be distinct");

    then.each_artifact_lists_only_its_own_destination(&[(slug_a, dest_a), (slug_b, dest_b)])
        .await?;

    Ok(())
}

/// Two annotations of the *same* artifact both show where that artifact went.
///
/// `annotations` is unique on `slug`, not on `artifact_id`, so a project can
/// hold several annotations naming one artifact — re-annotating a build that
/// was already released produces exactly that. The listing groups destination
/// rows by artifact and hands each annotation the group it matches, which is
/// only correct if the group can be read more than once; consuming it would
/// leave the second annotation rendering as a build that never deployed.
#[tokio::test(flavor = "multi_thread")]
async fn test_two_annotations_of_one_artifact_both_list_its_destinations() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let env = format!("accept-env-{suffix}");
    let dest = format!("accept-dest-{suffix}");

    given
        .clone()
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

    when.release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    let first_slug = then.data().slug.clone();

    // No upload in between, so this annotates the artifact that was just
    // released rather than a new one.
    given.an_annotated_release().await;
    let second_slug = then.data().slug.clone();
    assert_ne!(
        first_slug, second_slug,
        "annotating twice should produce two annotations"
    );

    then.each_artifact_lists_only_its_own_destination(&[
        (first_slug, dest.clone()),
        (second_slug, dest),
    ])
    .await?;

    Ok(())
}
