use forest_grpc_interface::{
    GetDestinationStatesRequest, GetReleaseIntentStatesRequest, PipelineRunStageStatus,
    ReleaseIntentState,
};
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(inner);
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid token metadata"),
    );
    request
}

struct SoakStatus {
    fixture: Fixture,
    token: String,
    organisation: String,
    project_id: Uuid,
    artifact_id: Uuid,
    annotation_id: Uuid,
    source_destination_id: Uuid,
    target_destination_id: Uuid,
    policy_id: Uuid,
}

impl SoakStatus {
    async fn new() -> anyhow::Result<Self> {
        let (given, _, _) = testcase::<ReleaseFlowData>().await?;
        let organisation = format!("test-org-{}", Uuid::now_v7());
        let given = given
            .a_registered_user()
            .await
            .an_organisation(&organisation)
            .await
            .an_environment("source")
            .await
            .an_environment("target")
            .await
            .a_destination_of_type("source-dest", "source", "forest/noop@1")
            .await
            .a_destination_of_type("target-dest", "target", "forest/noop@1")
            .await
            .an_uploaded_artifact()
            .await
            .an_annotated_release()
            .await;
        let fixture = given.fixture().clone();
        let token = given.data().auth_token.clone();
        let artifact_id: Uuid = given.data().artifact_id.parse()?;
        let (annotation_id, project_id): (Uuid, Uuid) =
            sqlx::query_as("SELECT id, project_id FROM annotations WHERE artifact_id = $1")
                .bind(artifact_id)
                .fetch_one(&fixture.db)
                .await?;
        let source_destination_id = sqlx::query_scalar(
            "SELECT id FROM destinations WHERE organisation = $1 AND name = 'source-dest'",
        )
        .bind(&organisation)
        .fetch_one(&fixture.db)
        .await?;
        let target_destination_id = sqlx::query_scalar(
            "SELECT id FROM destinations WHERE organisation = $1 AND name = 'target-dest'",
        )
        .bind(&organisation)
        .fetch_one(&fixture.db)
        .await?;
        let policy_id = sqlx::query_scalar(
            "INSERT INTO policies (project_id, name, policy_type, config)
             VALUES ($1, 'source-soak', 'soak_time', $2) RETURNING id",
        )
        .bind(project_id)
        .bind(serde_json::json!({
            "source_environment": "source",
            "target_environment": "target",
            "duration_seconds": 3600,
        }))
        .fetch_one(&fixture.db)
        .await?;
        Ok(Self {
            fixture,
            token,
            organisation,
            project_id,
            artifact_id,
            annotation_id,
            source_destination_id,
            target_destination_id,
            policy_id,
        })
    }

    async fn intent(&self, stages: Option<serde_json::Value>) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        let stage_states = stages.as_ref().map(|_| {
            serde_json::json!({
                "ready-deploy": {"status": "PENDING"},
                "ready-plan": {"status": "PENDING"},
                "dependent": {"status": "PENDING"},
            })
        });
        sqlx::query(
            "INSERT INTO release_intents
                 (id, artifact, annotation_id, project_id, status, stages, stage_states, next_evaluate_at)
             VALUES ($1, $2, $3, $4, 'ACTIVE', $5, $6, now() + interval '1 hour')",
        )
        .bind(id)
        .bind(self.artifact_id)
        .bind(self.annotation_id)
        .bind(self.project_id)
        .bind(stages)
        .bind(stage_states)
        .execute(&self.fixture.db)
        .await?;
        Ok(id)
    }

    async fn queued_target(&self, intent: Uuid) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO release_states
                 (release_id, release_intent_id, project_id, destination_id, artifact_id, status)
             VALUES ($1, $2, $3, $4, $5, 'QUEUED')",
        )
        .bind(id)
        .bind(intent)
        .bind(self.project_id)
        .bind(self.target_destination_id)
        .bind(self.artifact_id)
        .execute(&self.fixture.db)
        .await?;
        Ok(id)
    }

    async fn source_success(&self, artifact: Uuid, mode: &str) -> anyhow::Result<()> {
        let intent = self.intent(None).await?;
        sqlx::query("UPDATE release_intents SET artifact = $2, status = 'COMPLETED' WHERE id = $1")
            .bind(intent)
            .bind(artifact)
            .execute(&self.fixture.db)
            .await?;
        sqlx::query(
            "INSERT INTO release_states
                 (release_id, release_intent_id, project_id, destination_id, artifact_id,
                  status, mode, completed_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, 'SUCCEEDED', $6,
                     now() - interval '2 hours', now() - interval '2 hours')",
        )
        .bind(Uuid::now_v7())
        .bind(intent)
        .bind(self.project_id)
        .bind(self.source_destination_id)
        .bind(artifact)
        .bind(mode)
        .execute(&self.fixture.db)
        .await?;
        Ok(())
    }

    async fn status(&self, intent: Uuid) -> anyhow::Result<ReleaseIntentState> {
        let response = self
            .fixture
            .releases()
            .get_release_intent_states(authed_request(
                &self.token,
                GetReleaseIntentStatesRequest {
                    organisation: self.organisation.clone(),
                    project: Some("test-project".into()),
                    include_completed: true,
                },
            ))
            .await?
            .into_inner();
        let intent = intent.to_string();
        response
            .release_intents
            .into_iter()
            .find(|state| state.release_intent_id == intent)
            .ok_or_else(|| anyhow::anyhow!("intent absent from release status"))
    }

    async fn destination_reason(&self, release: Uuid) -> anyhow::Result<Option<String>> {
        let response = self
            .fixture
            .releases()
            .get_destination_states(authed_request(
                &self.token,
                GetDestinationStatesRequest {
                    organisation: self.organisation.clone(),
                    project: Some("test-project".into()),
                },
            ))
            .await?
            .into_inner();
        let release = release.to_string();
        Ok(response
            .destinations
            .into_iter()
            .find(|state| state.release_id.as_deref() == Some(release.as_str()))
            .expect("release destination status")
            .error_message)
    }
}

fn assert_missing_source(reason: Option<&str>) {
    let reason = reason.expect("artifact-specific blocker");
    assert!(reason.contains("this artifact"), "{reason}");
    assert!(reason.contains("no successful deploy"), "{reason}");
    assert!(reason.contains("source"), "{reason}");
    assert!(reason.contains("3600s"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_status_tracks_this_artifacts_successful_source_deploy() -> anyhow::Result<()> {
    let soak = SoakStatus::new().await?;
    let intent = soak.intent(None).await?;
    let release = soak.queued_target(intent).await?;
    let mut dispatch_guard = soak.fixture.db.begin().await?;
    sqlx::query("SELECT release_id FROM release_states WHERE release_id = $1 FOR UPDATE")
        .bind(release)
        .fetch_one(&mut *dispatch_guard)
        .await?;

    let state = soak.status(intent).await?;
    assert_eq!(state.steps[0].status, "QUEUED");
    assert_missing_source(state.steps[0].error_message.as_deref());
    assert_missing_source(soak.destination_reason(release).await?.as_deref());

    soak.source_success(soak.artifact_id, "plan").await?;
    soak.source_success(Uuid::now_v7(), "deploy").await?;
    assert_missing_source(soak.status(intent).await?.steps[0].error_message.as_deref());
    assert_missing_source(soak.destination_reason(release).await?.as_deref());

    soak.source_success(soak.artifact_id, "deploy").await?;
    assert!(soak.status(intent).await?.steps[0].error_message.is_none());
    assert!(soak.destination_reason(release).await?.is_none());
    let stored: Option<String> =
        sqlx::query_scalar("SELECT error_message FROM release_states WHERE release_id = $1")
            .bind(release)
            .fetch_one(&soak.fixture.db)
            .await?;
    assert!(stored.is_none());
    dispatch_guard.rollback().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn only_ready_pending_stages_report_the_current_soak_blocker() -> anyhow::Result<()> {
    let soak = SoakStatus::new().await?;
    let intent = soak
        .intent(Some(serde_json::json!({
            "ready-deploy": {"type": "deploy", "environment": "target"},
            "ready-plan": {"type": "plan", "environment": "target"},
            "dependent": {
                "type": "deploy", "environment": "target", "depends_on": ["ready-deploy"]
            },
        })))
        .await?;
    let state = soak.status(intent).await?;
    for stage in &state.stages {
        assert_eq!(stage.status, PipelineRunStageStatus::Pending as i32);
        if stage.stage_id == "dependent" {
            assert!(stage.error_message.is_none());
        } else {
            assert_missing_source(stage.error_message.as_deref());
        }
    }

    sqlx::query("UPDATE policies SET config = $2 WHERE id = $1")
        .bind(soak.policy_id)
        .bind(serde_json::json!({"invalid": "private-provider-detail"}))
        .execute(&soak.fixture.db)
        .await?;
    for stage in soak.status(intent).await?.stages {
        if stage.stage_id == "dependent" {
            assert!(stage.error_message.is_none());
        } else {
            let reason = stage.error_message.expect("policy evaluation blocker");
            assert!(reason.contains("policies could not be evaluated"));
            assert!(!reason.contains("private-provider-detail"));
        }
    }

    sqlx::query("UPDATE policies SET enabled = false WHERE id = $1")
        .bind(soak.policy_id)
        .execute(&soak.fixture.db)
        .await?;
    assert!(
        soak.status(intent)
            .await?
            .stages
            .iter()
            .all(|stage| stage.error_message.is_none())
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluation_errors_are_blockers_not_approval_or_stale_terminal_errors() -> anyhow::Result<()>
{
    let soak = SoakStatus::new().await?;
    let intent = soak.intent(None).await?;
    let release = soak.queued_target(intent).await?;
    sqlx::query("UPDATE policies SET config = $2 WHERE id = $1")
        .bind(soak.policy_id)
        .bind(serde_json::json!({"invalid": "private-provider-detail"}))
        .execute(&soak.fixture.db)
        .await?;

    let state = soak.status(intent).await?;
    assert_eq!(state.steps[0].status, "QUEUED");
    let reason = state.steps[0]
        .error_message
        .as_deref()
        .expect("error blocker");
    assert!(reason.contains("policies could not be evaluated"));
    assert!(!reason.contains("private-provider-detail"));
    assert_eq!(
        soak.destination_reason(release).await?.as_deref(),
        Some(reason)
    );

    sqlx::query("UPDATE release_states SET status = 'ASSIGNED' WHERE release_id = $1")
        .bind(release)
        .execute(&soak.fixture.db)
        .await?;
    assert!(soak.status(intent).await?.steps[0].error_message.is_none());
    assert!(soak.destination_reason(release).await?.is_none());

    sqlx::query(
        "UPDATE release_states SET status = 'FAILED', error_message = 'execution failed',
             completed_at = now() WHERE release_id = $1",
    )
    .bind(release)
    .execute(&soak.fixture.db)
    .await?;
    assert_eq!(
        soak.status(intent).await?.steps[0].error_message.as_deref(),
        Some("execution failed")
    );
    assert_eq!(
        soak.destination_reason(release).await?.as_deref(),
        Some("execution failed")
    );
    Ok(())
}
