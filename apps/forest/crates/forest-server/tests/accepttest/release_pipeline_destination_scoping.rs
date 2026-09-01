//! A pipeline deploy stage releases to the destinations the project declared.
//!
//! `release_destination_scoping.rs` covers this property on the *request* path,
//! where the declaration arrives as an argument (`--destination`). The pipeline
//! path has no caller to pass it one: the stage is defined server-side and
//! activated by the coordinator, possibly long after the request and possibly
//! for a release nobody typed a command for. It resolved `deploy(<environment>)`
//! to every destination in the environment and stopped there.
//!
//! Found the hard way: `canopy-data-gateway` declared `^data-prod/.*$` for an
//! environment holding an ECS place *and* a shiitake slice registry. Both got
//! scheduled. The slice registry failed, which is the only reason anyone
//! noticed — against a destination type that tolerates whatever it is handed,
//! the same wrong decision is silent.
//!
//! The declaration reaches the coordinator via `annotations.deployment_items`,
//! parsed at annotate time from the deployment files the artifact carries. So
//! these tests go through the real annotate path rather than seeding the column.

use forest_grpc_interface::{
    DeployStageConfig, PipelineStage, PlanStageConfig, Project, ReleaseRequest, pipeline_stage,
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

fn project(org: &str) -> Project {
    Project {
        organisation: org.into(),
        project: "test-project".into(),
        readme: String::new(),
        description: String::new(),
        metadata: Some(Default::default()),
    }
}

fn deploy_stage(id: &str, environment: &str) -> PipelineStage {
    PipelineStage {
        id: id.into(),
        depends_on: vec![],
        config: Some(pipeline_stage::Config::Deploy(DeployStageConfig {
            environment: environment.into(),
        })),
    }
}

fn plan_stage(id: &str, environment: &str) -> PipelineStage {
    PipelineStage {
        id: id.into(),
        depends_on: vec![],
        config: Some(pipeline_stage::Config::Plan(PlanStageConfig {
            environment: environment.into(),
            auto_approve: true,
        })),
    }
}

/// Two destinations in one environment, an artifact declaring `selectors` for
/// it, a single-stage pipeline, and one coordinator evaluation. Returns the
/// destination names the stage scheduled, sorted.
///
/// `declare_for` is the environment the artifact's declaration names, which is
/// not always the environment the stage deploys — that difference is a case in
/// its own right (`canopy-hubspot-ingest` is in exactly that state today).
struct Outcome {
    scheduled: Vec<String>,
    stage_status: String,
    stage_error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn run_stage(
    stage: PipelineStage,
    stage_env: &str,
    declare_for: Option<(&str, &[&str])>,
    null_the_declaration: bool,
) -> anyhow::Result<Outcome> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let target = format!("target-{suffix}");
    let neighbour = format!("neighbour-{suffix}");

    // Two destinations, one environment — the shape that exposed the bug. An
    // environment holding one destination cannot tell the two behaviours apart,
    // which is why production stopped reproducing this after the workaround.
    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(stage_env)
        .await
        .a_destination(&target, stage_env)
        .await
        .a_destination(&neighbour, stage_env)
        .await;

    let given = match declare_for {
        Some((env, selectors)) => given.an_uploaded_artifact_declaring(env, selectors).await,
        // No item records at all — the project prepared nothing, so it declared
        // nothing. Distinct from NULL, which means "annotated before the column
        // existed"; both fan out, but only one of them is a statement.
        None => given.an_uploaded_artifact().await,
    };
    let given = given.an_annotated_release().await;

    let (token, artifact_id) = {
        let data = given.data();
        (data.auth_token.clone(), data.artifact_id.clone())
    };

    let stage_id = stage.id.clone();
    given
        .fixture()
        .release_pipelines()
        .create_release_pipeline(authed_request(
            &token,
            forest_grpc_interface::CreateReleasePipelineRequest {
                project: Some(project(&org)),
                name: format!("p-{suffix}"),
                stages: vec![stage],
            },
        ))
        .await
        .expect("create pipeline");

    if null_the_declaration {
        sqlx::query!(
            "UPDATE annotations SET deployment_items = NULL WHERE artifact_id = $1",
            artifact_id.parse::<uuid::Uuid>()?,
        )
        .execute(&given.fixture().db)
        .await?;
    }

    let resp = when
        .fixture()
        .releases()
        .release(authed_request(
            &token,
            ReleaseRequest {
                artifact_id: artifact_id.clone(),
                destinations: vec![],
                environments: vec![],
                force: false,
                use_pipeline: true,
                prepare_only: false,
            },
        ))
        .await?
        .into_inner();

    // A pipeline release has no per-destination releases yet, so the handler
    // surfaces the intent itself as a single entry with destination and
    // environment left blank (they are genuinely not known until a stage
    // activates).
    let intent_id: uuid::Uuid = resp
        .intents
        .first()
        .expect("a pipeline release returns its intent")
        .release_intent_id
        .parse()?;

    // The fixture does not run IntentCoordinator (see `evaluate`'s doc comment),
    // so drive the evaluation rather than sleeping on a 5s sweep.
    //
    // Retried rather than called once because the acceptance suite shares the dev
    // database: a `forest-server` running locally has its own coordinator
    // sweeping the same table, and its `FOR UPDATE SKIP LOCKED` makes our
    // `evaluate` a no-op while it holds the row. The stage still gets activated —
    // by the same code under test — so waiting for it to leave PENDING is correct
    // either way, and in the normal case the first call does it.
    let stage_id_ref = stage_id.as_str();
    // Budget deliberately generous (~15s): the suite is contended, and a run
    // that takes 35s rather than 6s should not turn into an assertion failure
    // about destination selection.
    for attempt in 0..60 {
        forest_server::intent_coordinator::evaluate(&when.fixture().state, intent_id).await?;

        let states = sqlx::query_scalar!(
            "SELECT stage_states FROM release_intents WHERE id = $1",
            intent_id,
        )
        .fetch_one(&when.fixture().db)
        .await?
        .unwrap_or(serde_json::Value::Null);

        if states[stage_id_ref]["status"].as_str() != Some("PENDING") {
            break;
        }

        assert!(attempt < 59, "stage {stage_id_ref} never left PENDING");
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let rows = sqlx::query!(
        r#"SELECT d.name as "name!"
           FROM release_states rs
           JOIN destinations d ON d.id = rs.destination_id
           WHERE rs.release_intent_id = $1"#,
        intent_id,
    )
    .fetch_all(&when.fixture().db)
    .await?;

    let mut scheduled: Vec<String> = rows.into_iter().map(|r| r.name).collect();
    scheduled.sort();

    let state = sqlx::query!(
        "SELECT stage_states FROM release_intents WHERE id = $1",
        intent_id,
    )
    .fetch_one(&when.fixture().db)
    .await?;

    let states: serde_json::Value = state.stage_states.unwrap_or(serde_json::Value::Null);
    let entry = &states[&stage_id];

    Ok(Outcome {
        scheduled,
        stage_status: entry["status"].as_str().unwrap_or("MISSING").to_string(),
        stage_error: entry["error_message"].as_str().map(str::to_string),
    })
}

/// Names the target so it can be compared against what was scheduled — the
/// suffix is generated inside `run_stage`, so tests match on shape instead.
fn only(scheduled: &[String], prefix: &str) -> bool {
    scheduled.len() == 1 && scheduled[0].starts_with(prefix)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_stage_schedules_only_the_destinations_the_project_declared() -> anyhow::Result<()>
{
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        deploy_stage("only", &env),
        &env,
        Some((&env, &["^target-.*$"])),
        false,
    )
    .await?;

    assert!(
        only(&outcome.scheduled, "target-"),
        "declaring ^target-.*$ should schedule the target alone; the environment \
         must not pull in the neighbour. scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

/// The other half of the contract, and the reason this change is safe to ship:
/// a project that declares no destinations for the stage's environment keeps
/// releasing to all of it. `infrastructure-hetzner` and the other pipeline
/// projects depend on this.
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_the_project_declares_nothing_for_still_fans_out() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(deploy_stage("only", &env), &env, None, false).await?;

    assert_eq!(
        outcome.scheduled.len(),
        2,
        "an artifact declaring nothing should still reach every destination in \
         the environment. scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

/// Declaring for a *different* environment is the same thing as declaring
/// nothing for this one. Not hypothetical: `canopy-hubspot-ingest`'s forest.cue
/// declares `data` while its pipeline deploys `data-prod`.
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_for_another_environment_does_not_narrow_this_one() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        deploy_stage("only", &env),
        &env,
        Some(("some-other-env", &["^target-.*$"])),
        false,
    )
    .await?;

    assert_eq!(
        outcome.scheduled.len(),
        2,
        "a selector scoped to another environment must not filter this one. \
         scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

/// The migration's read-side default. Distinct from the `[]` case above: that
/// one is "prepared and declared nothing", this one is "annotated before the
/// column existed", and they are different code paths.
#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_annotated_before_this_change_still_fans_out() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        deploy_stage("only", &env),
        &env,
        Some((&env, &["^target-.*$"])),
        true,
    )
    .await?;

    assert_eq!(
        outcome.scheduled.len(),
        2,
        "a NULL declaration must behave as it did before the column existed. \
         scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

/// Silent success having deployed nothing is the failure mode this destination
/// type was built to avoid, so both halves are asserted: the stage failed, *and*
/// nothing was scheduled.
#[tokio::test(flavor = "multi_thread")]
async fn a_selector_matching_nothing_fails_the_stage_and_deploys_nothing() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        deploy_stage("only", &env),
        &env,
        Some((&env, &["^nowhere/.*$"])),
        false,
    )
    .await?;

    assert!(
        outcome.scheduled.is_empty(),
        "nothing may be scheduled when no destination matches: {:?}",
        outcome.scheduled,
    );
    assert_eq!(outcome.stage_status, "FAILED");

    // The message has to send the reader somewhere. Naming the environment and
    // the selector is the difference between "which of my selectors, in which
    // environment" and reading the release log twice.
    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        error.contains(&env) && error.contains("^nowhere/.*$"),
        "the failure should name the environment and the selector, got: {error}",
    );

    Ok(())
}

/// A selector that matches nothing while a sibling matches something is a
/// warning, not a failure: a regex is allowed not to match, and a project
/// declaring several regions should not break because one does not exist here
/// yet.
#[tokio::test(flavor = "multi_thread")]
async fn a_selector_that_matches_some_does_not_fail_on_the_rest() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        deploy_stage("only", &env),
        &env,
        Some((&env, &["^target-.*$", "^nowhere/.*$"])),
        false,
    )
    .await?;

    assert!(
        only(&outcome.scheduled, "target-"),
        "the matching selector should still be honoured: {:?}",
        outcome.scheduled,
    );

    // Deliberately not asserting the stage is non-FAILED: a stage's terminal
    // status follows its releases, and a release in this environment fails on
    // arrival (the fixture's destinations are `forest/flux@1` pointed at a temp
    // dir). What matters is that it was not failed *for selection reasons* —
    // that message is the one `resolve_stage_destinations` produces when a
    // declaration matches nothing.
    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        !error.contains("match none of them"),
        "a partly-matching declaration must not be treated as matching nothing, \
         got: {error}",
    );

    Ok(())
}

/// The second call site. The Plan arm carried a byte-identical copy of the
/// environment query, so it inherited the bug silently; it now shares the
/// resolver, and this test is what keeps it sharing it.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_stage_honours_the_declaration_too() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_stage(
        plan_stage("only", &env),
        &env,
        Some((&env, &["^target-.*$"])),
        false,
    )
    .await?;

    assert!(
        only(&outcome.scheduled, "target-"),
        "a plan stage must narrow the same way a deploy stage does: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

/// The write side: annotate records what the project declared, untruncated.
/// `artifact_files.destination` records the same selector cut at its first `/`,
/// which is why scheduling reads this column and not that one.
#[tokio::test(flavor = "multi_thread")]
async fn annotate_records_the_declaration_with_the_selector_whole() -> anyhow::Result<()> {
    let (given, _when, _then) = testcase::<ReleaseFlowData>().await?;

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
        .an_uploaded_artifact_declaring(&env, &["^data-prod/.*$"])
        .await
        .an_annotated_release()
        .await;

    let artifact_id: uuid::Uuid = given.data().artifact_id.parse()?;

    let row = sqlx::query!(
        "SELECT deployment_items FROM annotations WHERE artifact_id = $1",
        artifact_id,
    )
    .fetch_one(&given.fixture().db)
    .await?;

    let items = row.deployment_items.expect("declaration recorded");
    assert_eq!(
        items[0]["destination"].as_str(),
        Some("^data-prod/.*$"),
        "the selector must survive whole; got {items}",
    );
    assert_eq!(items[0]["env"].as_str(), Some(env.as_str()));

    Ok(())
}
