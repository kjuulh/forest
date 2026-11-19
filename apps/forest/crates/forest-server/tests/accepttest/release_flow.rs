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

    when.release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    then.release_is_in_terminal_state()
        .await?
        .artifact_is_retrievable_by_slug()
        .await?
        .artifact_is_listed_in_project()
        .await?;

    Ok(())
}

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

#[tokio::test(flavor = "multi_thread")]
async fn test_a_reaped_release_announces_its_failure() -> anyhow::Result<()> {
    let (given, when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let dest = format!("reap-dest-{suffix}");
    let env = format!("reap-env-{suffix}");
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
    let when = when
        .release_is_triggered()
        .await?
        .release_reaches_terminal_state()
        .await?;

    let fixture = then.fixture();
    let artifact_id: uuid::Uuid = when.data().artifact_id.parse()?;
    sqlx::query(
        "UPDATE release_states
         SET status = 'RUNNING', last_heartbeat_at = now() - interval '10 minutes'
         WHERE artifact_id = $1",
    )
    .bind(artifact_id)
    .execute(&fixture.db)
    .await?;
    sqlx::query("DELETE FROM notifications WHERE organisation = $1")
        .bind(&org)
        .execute(&fixture.db)
        .await?;

    forest_server::release_reaper::ReleaseReaper::new(
        &fixture.state,
        forest_server::runner_manager::RunnerManager::new(),
    )
    .reap()
    .await?;

    let failed: Vec<(String, String)> =
        sqlx::query_as("SELECT notification_type, body FROM notifications WHERE organisation = $1")
            .bind(&org)
            .fetch_all(&fixture.db)
            .await?;
    assert_eq!(
        failed.len(),
        1,
        "one notification for the reaped release: {failed:?}"
    );
    assert_eq!(failed[0].0, "RELEASE_FAILED");
    assert!(failed[0].1.contains("heartbeat lost"), "{}", failed[0].1);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_a_release_a_trigger_starts_announces_that_it_started() -> anyhow::Result<()> {
    use forest_grpc_interface::*;

    let (given, _when, then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let dest = format!("trig-dest-{suffix}");
    let env = format!("trig-env-{suffix}");
    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&env)
        .await
        .a_destination(&dest, &env)
        .await
        .an_uploaded_artifact()
        .await;

    let fixture = then.fixture();
    let token = given.data().auth_token.clone();
    let authed = |inner| {
        let mut req = tonic::Request::new(inner);
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid metadata"),
        );
        req
    };
    fixture
        .releases()
        .create_project(authed(CreateProjectRequest {
            organisation: org.clone(),
            project: "test-project".into(),
        }))
        .await?;
    let mut triggers = trigger_service_client::TriggerServiceClient::new(fixture.channel.clone());
    let mut req = tonic::Request::new(CreateTriggerRequest {
        project: Some(Project {
            organisation: org.clone(),
            project: "test-project".into(),
            readme: String::new(),
            description: String::new(),
            metadata: Some(Default::default()),
        }),
        name: "main".into(),
        branch_pattern: Some("^main$".into()),
        target_destinations: vec![dest.clone()],
        ..Default::default()
    });
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", given.data().auth_token).parse()?,
    );
    triggers.create_trigger(req).await?;

    given.an_annotated_release().await;

    let started: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT notification_type, release_context FROM notifications
         WHERE organisation = $1 AND notification_type = 'RELEASE_STARTED'",
    )
    .bind(&org)
    .fetch_all(&fixture.db)
    .await?;
    assert_eq!(
        started.len(),
        1,
        "the trigger's release announces its start: {started:?}"
    );
    let intent = started[0].1["release_intent_id"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !intent.is_empty(),
        "the start names its intent: {:?}",
        started[0].1
    );

    Ok(())
}
