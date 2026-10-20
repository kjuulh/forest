use forest_server::services::policy::{PolicyRegistryState, PolicyType};
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, GivenReleaseFlow, fixture, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

struct Soak {
    fixture: Fixture,
    project_id: Uuid,
    source_destination: String,
    target: String,
}

static PROJECT: tokio::sync::OnceCell<(Uuid, String, String)> = tokio::sync::OnceCell::const_new();

async fn a_soak_policy(duration_seconds: i64) -> anyhow::Result<Soak> {
    let (project_id, source_env, source_destination) = PROJECT
        .get_or_try_init(|| async {
            let (given, _when, _then) = testcase::<ReleaseFlowData>().await?;

            let suffix = Uuid::now_v7();
            let org = format!("test-org-{suffix}");
            let env = format!("soak-source-{suffix}");
            let dest = format!("soak-dest-{suffix}");

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

            let f = fixture().await?;
            let project_id = sqlx::query_scalar!(
                "SELECT id FROM projects WHERE organisation = $1 AND project = 'test-project'",
                org,
            )
            .fetch_one(&f.db)
            .await?;

            Ok::<_, anyhow::Error>((project_id, env, dest))
        })
        .await?
        .clone();
    let f = fixture().await?;

    let target = format!("soak-target-{}", Uuid::now_v7());
    sqlx::query!(
        r#"INSERT INTO policies (project_id, name, policy_type, config)
           VALUES ($1, $2, 'soak_time', $3)"#,
        project_id,
        format!("soak-{target}"),
        serde_json::json!({
            "source_environment": source_env,
            "target_environment": target,
            "duration_seconds": duration_seconds,
        }),
    )
    .execute(&f.db)
    .await?;

    Ok(Soak {
        fixture: f,
        project_id,
        source_destination,
        target,
    })
}

impl Soak {
    async fn succeeded_in_source(&self, artifact: Uuid, mode: &str, hours_ago: i64) {
        let destination_id = sqlx::query_scalar!(
            "SELECT id FROM destinations WHERE name = $1",
            self.source_destination
        )
        .fetch_one(&self.fixture.db)
        .await
        .expect("source destination");

        let intent_id = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_intents (id, artifact, annotation_id, project_id, status)
               VALUES ($1, $2, $3, $4, 'COMPLETED')"#,
            intent_id,
            artifact,
            Uuid::now_v7(),
            self.project_id,
        )
        .execute(&self.fixture.db)
        .await
        .expect("insert intent");

        sqlx::query!(
            r#"INSERT INTO release_states (
                   release_id, release_intent_id, project_id, destination_id,
                   artifact_id, status, mode, queued_at, completed_at, updated_at
               ) VALUES (
                   $1, $2, $3, $4, $5, 'SUCCEEDED', $6,
                   now() - ($7 || ' hours')::interval,
                   now() - ($7 || ' hours')::interval,
                   now() - ($7 || ' hours')::interval
               )"#,
            Uuid::now_v7(),
            intent_id,
            self.project_id,
            destination_id,
            artifact,
            mode,
            hours_ago.to_string(),
        )
        .execute(&self.fixture.db)
        .await
        .expect("insert release state");
    }

    async fn passes_for(&self, artifact: Uuid) -> (bool, String) {
        let evaluations = self
            .fixture
            .state
            .policy_registry()
            .evaluate_for_environment(&self.project_id, &self.target, None, None, Some(&artifact))
            .await
            .expect("evaluate");
        let soak = evaluations
            .into_iter()
            .find(|e| e.policy_type == PolicyType::SoakTime)
            .expect("a soak evaluation");
        (soak.passed, soak.reason)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_soak_is_satisfied_by_this_artifacts_deploy_to_the_source() -> anyhow::Result<()> {
    let soak = a_soak_policy(3600).await?;
    let artifact = Uuid::now_v7();
    soak.succeeded_in_source(artifact, "deploy", 2).await;

    let (passed, reason) = soak.passes_for(artifact).await;

    assert!(passed, "{reason}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_plan_in_the_source_does_not_count_as_soak() -> anyhow::Result<()> {
    let soak = a_soak_policy(3600).await?;
    let artifact = Uuid::now_v7();
    soak.succeeded_in_source(artifact, "plan", 2).await;

    let (passed, reason) = soak.passes_for(artifact).await;

    assert!(!passed, "{reason}");
    assert!(reason.contains("no successful deploy"), "{reason}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn another_artifacts_deploy_does_not_soak_this_one() -> anyhow::Result<()> {
    let soak = a_soak_policy(3600).await?;
    soak.succeeded_in_source(Uuid::now_v7(), "deploy", 2).await;

    let (passed, reason) = soak.passes_for(Uuid::now_v7()).await;

    assert!(!passed, "{reason}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_younger_than_the_duration_is_not_yet_soaked() -> anyhow::Result<()> {
    let soak = a_soak_policy(3 * 3600).await?;
    let artifact = Uuid::now_v7();
    soak.succeeded_in_source(artifact, "deploy", 1).await;

    let (passed, reason) = soak.passes_for(artifact).await;

    assert!(!passed, "{reason}");
    assert!(reason.contains("remaining"), "{reason}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn named_destinations_resolve_to_their_environments() -> anyhow::Result<()> {
    let soak = a_soak_policy(60).await?;

    let environments = soak
        .fixture
        .state
        .policy_registry()
        .environments_of_destinations(
            &soak.project_id,
            &[
                soak.source_destination.clone(),
                "no-such-destination".into(),
            ],
        )
        .await?;

    assert_eq!(environments.len(), 1);
    assert!(environments[0].starts_with("soak-source-"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_release_cannot_bypass_a_new_branch_restriction() -> anyhow::Result<()> {
    use crate::accepttest::fixtures::WhenReleaseFlow;

    let (given, when, then) = testcase::<ReleaseFlowData>().await?;
    let suffix = Uuid::now_v7();
    let org = format!("dispatch-policy-{suffix}");
    let target = format!("target-{suffix}");
    let destination = format!("noop-{suffix}");
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&target)
        .await
        .a_destination_of_type(&destination, &target, "forest/noop@1")
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;
    let f = fixture().await?;
    let artifact: Uuid = then.data().artifact_id.parse()?;
    let project: Uuid = sqlx::query_scalar(
        "UPDATE annotations SET ref = jsonb_set(ref, '{commit_branch}', '\"feature/review\"')
         WHERE artifact_id = $1 RETURNING project_id",
    )
    .bind(artifact)
    .fetch_one(&f.db)
    .await?;
    sqlx::query(
        "INSERT INTO policies (project_id, name, policy_type, config)
         VALUES ($1, 'hold', 'soak_time', $2)",
    )
    .bind(project)
    .bind(serde_json::json!({
        "source_environment": "not-yet-deployed",
        "target_environment": target,
        "duration_seconds": 3600,
    }))
    .execute(&f.db)
    .await?;

    when.release_is_triggered().await?;
    let intent: Uuid = then.data().release_intent_id.parse()?;
    let release: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM release_states WHERE release_intent_id = $1 AND status = 'QUEUED'",
    )
    .bind(intent)
    .fetch_one(&f.db)
    .await?;
    sqlx::query(
        "INSERT INTO policies (project_id, name, policy_type, config)
         VALUES ($1, 'main-only', 'branch_restriction', $2)",
    )
    .bind(project)
    .bind(serde_json::json!({
        "target_environment": target,
        "branch_pattern": "^main$",
    }))
    .execute(&f.db)
    .await?;
    f.state
        .nats
        .publish("forest.release.queued", release.to_string().into())
        .await?;
    f.state.nats.flush().await?;

    let (status, reason): (String, Option<String>) =
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let row: (String, Option<String>) = sqlx::query_as(
                    "SELECT status, error_message FROM release_states WHERE release_id = $1",
                )
                .bind(release)
                .fetch_one(&f.db)
                .await?;
                if row.0 != "QUEUED" {
                    return Ok::<_, anyhow::Error>(row);
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await??;
    assert_eq!(status, "FAILED");
    let reason = reason.expect("the branch refusal is persisted");
    assert!(reason.contains("blocked by policy 'main-only'"), "{reason}");
    assert!(reason.contains("feature/review"), "{reason}");
    let assigned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM release_events
         WHERE release_id = $1 AND event_type = 'release.assigned'",
    )
    .bind(release)
    .fetch_one(&f.db)
    .await?;
    assert_eq!(assigned, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restricted_environment_requires_recorded_branch_metadata() -> anyhow::Result<()> {
    let (given, _when, then) = testcase::<ReleaseFlowData>().await?;
    let suffix = Uuid::now_v7();
    let org = format!("missing-branch-{suffix}");
    let target = format!("target-{suffix}");
    let destination = format!("noop-{suffix}");
    given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(&target)
        .await
        .a_destination_of_type(&destination, &target, "forest/noop@1")
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;
    let (artifact, token) = {
        let data = then.data();
        (data.artifact_id.parse::<Uuid>()?, data.auth_token.clone())
    };
    let f = fixture().await?;
    let project: Uuid =
        sqlx::query_scalar("SELECT project_id FROM annotations WHERE artifact_id = $1")
            .bind(artifact)
            .fetch_one(&f.db)
            .await?;
    sqlx::query(
        "INSERT INTO policies (project_id, name, policy_type, config)
         VALUES ($1, 'requires-branch', 'branch_restriction', $2)",
    )
    .bind(project)
    .bind(serde_json::json!({
        "target_environment": target,
        "branch_pattern": ".*",
    }))
    .execute(&f.db)
    .await?;

    let request = || {
        let mut request = tonic::Request::new(forest_grpc_interface::ReleaseRequest {
            artifact_id: artifact.to_string(),
            destinations: vec![destination.clone()],
            environments: vec![],
            force: false,
            use_pipeline: false,
            prepare_only: false,
        });
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    };
    for branch in [serde_json::Value::Null, serde_json::json!("")] {
        sqlx::query(
            "UPDATE annotations SET ref = jsonb_set(ref, '{commit_branch}', $2)
             WHERE artifact_id = $1",
        )
        .bind(artifact)
        .bind(branch)
        .execute(&f.db)
        .await?;
        let error = f.releases().release(request()).await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
        assert!(error.message().contains("requires-branch"));
    }
    let intents: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM release_intents WHERE artifact = $1")
            .bind(artifact)
            .fetch_one(&f.db)
            .await?;
    assert_eq!(intents, 0);

    sqlx::query("UPDATE policies SET enabled = false WHERE project_id = $1")
        .bind(project)
        .execute(&f.db)
        .await?;
    sqlx::query("UPDATE annotations SET ref = ref - 'commit_branch' WHERE artifact_id = $1")
        .bind(artifact)
        .execute(&f.db)
        .await?;
    let response = f.releases().release(request()).await?.into_inner();
    assert_eq!(response.intents.len(), 1);
    let branch: Option<String> =
        sqlx::query_scalar("SELECT ref->>'commit_branch' FROM annotations WHERE artifact_id = $1")
            .bind(artifact)
            .fetch_one(&f.db)
            .await?;
    assert!(branch.is_none());
    Ok(())
}
