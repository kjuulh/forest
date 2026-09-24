//! `WaitRelease` on a release parked awaiting plan approval.
//!
//! The stream stays open until the release is terminal, and a parked release
//! is not terminal until someone approves the plan. `forest release show`
//! read the stream to the end before printing anything, so it printed the
//! header and hung: the person asked to approve a plan could not see it.
//!
//! The server now says when its first pass (the replay of everything persisted)
//! is over, and says what each stage waits on, so a client can stop at the
//! replay or tell "parked" from "about to run". Found on 2026-09-24 with
//! `unhappily-trim-brill` in `kjuulh/grund-cloudflare`.

use forest_grpc_interface::{
    DeployStageConfig, PipelineStage, PlanStageConfig, Project, ReleaseRequest, WaitReleaseEvent,
    WaitReleaseRequest, pipeline_stage, wait_release_event::Event,
};

use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

/// Local copy: the identical helper is module-private in each flow module, and
/// they each keep their own rather than widening it.
fn authed_request<T>(token: &str, inner: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(inner);
    let val: tonic::metadata::MetadataValue<_> =
        format!("Bearer {token}").parse().expect("valid metadata");
    req.metadata_mut().insert("authorization", val);
    req
}

/// A pipeline release whose plan stage has run and parked awaiting approval,
/// with a deploy stage behind it. Returns the token and the intent id.
async fn a_release_parked_on_plan_approval() -> anyhow::Result<(
    crate::accepttest::fixtures::Given<ReleaseFlowData>,
    String,
    uuid::Uuid,
)> {
    let (given, _when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment("prod")
        .await
        .a_destination_of_type(&format!("infra-{suffix}"), "prod", "forest/terraform@1")
        .await
        .an_uploaded_artifact()
        .await
        .an_annotated_release()
        .await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    given
        .fixture()
        .release_pipelines()
        .create_release_pipeline(authed_request(
            &token,
            forest_grpc_interface::CreateReleasePipelineRequest {
                project: Some(Project {
                    organisation: org.clone(),
                    project: "test-project".into(),
                    readme: String::new(),
                    description: String::new(),
                    metadata: Some(Default::default()),
                }),
                name: format!("p-{suffix}"),
                stages: vec![
                    PipelineStage {
                        id: "plan-prod".into(),
                        depends_on: vec![],
                        config: Some(pipeline_stage::Config::Plan(PlanStageConfig {
                            environment: "prod".into(),
                            auto_approve: false,
                        })),
                    },
                    PipelineStage {
                        id: "deploy-prod".into(),
                        depends_on: vec!["plan-prod".into()],
                        config: Some(pipeline_stage::Config::Deploy(DeployStageConfig {
                            environment: "prod".into(),
                        })),
                    },
                ],
            },
        ))
        .await
        .expect("create pipeline");

    let resp = given
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id,
                destinations: vec![],
                environments: vec![],
                force: false,
                use_pipeline: true,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();
    let intent_id: uuid::Uuid = resp
        .intents
        .first()
        .expect("a pipeline release returns its intent")
        .release_intent_id
        .parse()?;

    // The fixture runs no coordinator and no runner, so put the intent where
    // they would leave it once the plan ran: the plan stage ACTIVE and awaiting
    // approval (approval is tracked beside the status, not as a value of it),
    // the deploy stage PENDING behind it.
    sqlx::query!(
        "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
        intent_id,
        serde_json::json!({
            "plan-prod": {
                "status": "ACTIVE",
                "started_at": "2026-09-24T21:12:30Z",
                "approval_status": "AWAITING_APPROVAL",
            },
            "deploy-prod": { "status": "PENDING" },
        }),
    )
    .execute(&given.fixture().db)
    .await?;

    Ok((given, token, intent_id))
}

async fn next_event(
    stream: &mut tonic::Streaming<WaitReleaseEvent>,
    within: std::time::Duration,
) -> Option<Option<WaitReleaseEvent>> {
    tokio::time::timeout(within, stream.message())
        .await
        .ok()
        .map(|m| m.expect("stream message"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parked_release_replays_its_stages_then_says_the_replay_is_complete()
-> anyhow::Result<()> {
    let (given, token, intent_id) = a_release_parked_on_plan_approval().await?;

    let mut stream = given
        .fixture()
        .releases()
        .wait_release(authed_request(
            &token,
            WaitReleaseRequest {
                release_intent_id: intent_id.to_string(),
            },
        ))
        .await?
        .into_inner();

    let mut stages = Vec::new();
    loop {
        let event = next_event(&mut stream, std::time::Duration::from_secs(10))
            .await
            .expect("the replay marker within 10s")
            .expect("the stream stays open for a parked release");
        match event.event {
            Some(Event::StageUpdate(stage)) => stages.push(stage),
            Some(Event::ReplayComplete(_)) => break,
            _ => {}
        }
    }

    stages.sort_by(|a, b| a.stage_id.cmp(&b.stage_id));
    let [deploy, plan] = stages.as_slice() else {
        panic!("both stages replayed before the marker, got {stages:?}");
    };
    assert_eq!(plan.stage_id, "plan-prod");
    assert_eq!(plan.status, "ACTIVE");
    assert_eq!(plan.approval_status.as_deref(), Some("AWAITING_APPROVAL"));
    assert_eq!(deploy.stage_id, "deploy-prod");
    assert_eq!(deploy.status, "PENDING");
    assert_eq!(
        deploy.depends_on,
        vec!["plan-prod".to_string()],
        "a follower needs the graph to tell a blocked stage from one about to run"
    );

    Ok(())
}

/// The marker is about the replay, not about every poll: sent once. And the
/// stream still does not end, because `show --follow` relies on it to wait for
/// the approval.
#[tokio::test(flavor = "multi_thread")]
async fn the_replay_marker_is_sent_once_and_the_stream_stays_open() -> anyhow::Result<()> {
    let (given, token, intent_id) = a_release_parked_on_plan_approval().await?;

    let mut stream = given
        .fixture()
        .releases()
        .wait_release(authed_request(
            &token,
            WaitReleaseRequest {
                release_intent_id: intent_id.to_string(),
            },
        ))
        .await?
        .into_inner();

    loop {
        let event = next_event(&mut stream, std::time::Duration::from_secs(10))
            .await
            .expect("the replay marker within 10s")
            .expect("the stream stays open for a parked release");
        if matches!(event.event, Some(Event::ReplayComplete(_))) {
            break;
        }
    }

    // Longer than the server's 2s poll, so at least one more pass runs. A
    // parked release has nothing new to say on it.
    let after = next_event(&mut stream, std::time::Duration::from_secs(5)).await;
    assert!(
        after.is_none(),
        "expected silence after the replay of a parked release, got {after:?}"
    );

    Ok(())
}
