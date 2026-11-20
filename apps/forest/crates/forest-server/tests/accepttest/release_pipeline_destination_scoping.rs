use forest_grpc_interface::{
    DeployStageConfig, PipelineStage, PlanStageConfig, Project, ReleaseRequest, pipeline_stage,
};

use crate::accepttest::fixtures::{GivenReleaseFlow, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

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
            destroy: false,
        })),
    }
}

struct Outcome {
    scheduled: Vec<String>,
    stage_status: String,
    stage_error: Option<String>,
}

const FLUX: &str = "forest/flux@1";
const TERRAFORM: &str = "forest/terraform@1";

#[allow(clippy::too_many_arguments)]
async fn run_stage(
    stage: PipelineStage,
    stage_env: &str,
    declare_for: Option<(&str, &[&str])>,
    null_the_declaration: bool,
) -> anyhow::Result<Outcome> {
    let declare_for = declare_for.map(|(env, selectors)| {
        (
            env,
            selectors
                .iter()
                .map(|selector| (*selector, FLUX))
                .collect::<Vec<_>>(),
        )
    });

    run_typed_stage(
        stage,
        stage_env,
        (FLUX, FLUX),
        declare_for
            .as_ref()
            .map(|(env, declarations)| (*env, declarations.as_slice())),
        null_the_declaration,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_typed_stage(
    stage: PipelineStage,
    stage_env: &str,
    types: (&str, &str),
    declare_for: Option<(&str, &[(&str, &str)])>,
    null_the_declaration: bool,
) -> anyhow::Result<Outcome> {
    let (given, when, _then) = testcase::<ReleaseFlowData>().await?;

    let suffix = uuid::Uuid::now_v7();
    let org = format!("test-org-{suffix}");
    let target = format!("target-{suffix}");
    let neighbour = format!("neighbour-{suffix}");

    let given = given
        .a_registered_user()
        .await
        .an_organisation(&org)
        .await
        .an_environment(stage_env)
        .await
        .a_destination_of_type(&target, stage_env, types.0)
        .await
        .a_destination_of_type(&neighbour, stage_env, types.1)
        .await;

    let given = match declare_for {
        Some((env, declarations)) => {
            given
                .an_uploaded_artifact_declaring_types(env, declarations)
                .await
        }
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

    let intent_id: uuid::Uuid = resp
        .intents
        .first()
        .expect("a pipeline release returns its intent")
        .release_intent_id
        .parse()?;

    let stage_id_ref = stage_id.as_str();
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

#[tokio::test(flavor = "multi_thread")]
async fn an_undeclared_environment_does_not_widen_onto_a_kind_the_artifact_never_renders()
-> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_typed_stage(
        deploy_stage("only", &env),
        &env,
        (FLUX, TERRAFORM),
        Some(("some-other-env", &[("^target-.*$", FLUX)])),
        false,
    )
    .await?;

    assert!(
        only(&outcome.scheduled, "target-"),
        "only the flux destination is of a kind this artifact renders. \
         scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

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

    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        error.contains(&env) && error.contains("^nowhere/.*$"),
        "the failure should name the environment and the selector, got: {error}",
    );

    Ok(())
}

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

    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        !error.contains("match none of them"),
        "a partly-matching declaration must not be treated as matching nothing, \
         got: {error}",
    );

    Ok(())
}

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

#[tokio::test(flavor = "multi_thread")]
async fn the_server_overrides_what_the_client_claimed_a_file_was_for() -> anyhow::Result<()> {
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
        .an_uploaded_artifact_declaring_mislabelled(&env, "^data-prod/.*$", (&env, "^data-prod"))
        .await;

    let artifact_id: uuid::Uuid = given.data().artifact_id.parse()?;

    let row = sqlx::query!(
        r#"SELECT f.env as "env!", f.destination as "destination!"
           FROM artifacts a
           JOIN artifact_files f ON f.artifact_staging_id = a.artifact_id
           WHERE a.id = $1 AND f.category = 'deployment'"#,
        artifact_id,
    )
    .fetch_one(&given.fixture().db)
    .await?;

    assert_eq!(
        row.destination, "^data-prod/.*$",
        "the server must record the selector the item declared, not the truncated \
         value the client claimed",
    );
    assert_eq!(row.env, env);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stage_does_not_schedule_a_destination_whose_type_the_project_never_declared()
-> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_typed_stage(
        deploy_stage("only", &env),
        &env,
        (TERRAFORM, FLUX),
        Some((&env, &[(".*", TERRAFORM)])),
        false,
    )
    .await?;

    assert!(
        only(&outcome.scheduled, "target-"),
        "a terraform declaration must reach the terraform destination and not \
         the flux one, even though the selector matches both names. \
         scheduled: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_of_an_undeclared_type_leaves_no_release_row() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_typed_stage(
        deploy_stage("only", &env),
        &env,
        (FLUX, TERRAFORM),
        Some((&env, &[(".*", FLUX)])),
        false,
    )
    .await?;

    assert_eq!(
        outcome.scheduled.len(),
        1,
        "the undeclared-type destination must not be scheduled at all: {:?}",
        outcome.scheduled,
    );

    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        !error.contains("match none of them"),
        "an out-of-scope destination is not a selection failure, got: {error}",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_declaring_both_types_still_reaches_both_destinations() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_typed_stage(
        deploy_stage("only", &env),
        &env,
        (TERRAFORM, FLUX),
        Some((
            &env,
            &[("^target-.*$", TERRAFORM), ("^neighbour-.*$", FLUX)],
        )),
        false,
    )
    .await?;

    assert_eq!(
        outcome.scheduled.len(),
        2,
        "declaring both kinds must schedule both: {:?}",
        outcome.scheduled,
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_matching_only_by_name_fails_and_says_why() -> anyhow::Result<()> {
    let env = format!("accept-env-{}", uuid::Uuid::now_v7());

    let outcome = run_typed_stage(
        deploy_stage("only", &env),
        &env,
        (FLUX, FLUX),
        Some((&env, &[(".*", TERRAFORM)])),
        false,
    )
    .await?;

    assert!(
        outcome.scheduled.is_empty(),
        "nothing may be scheduled: {:?}",
        outcome.scheduled,
    );
    assert_eq!(outcome.stage_status, "FAILED");

    let error = outcome.stage_error.unwrap_or_default();
    assert!(
        error.contains("a type this project declares nothing for"),
        "the failure should name the type mismatch, got: {error}",
    );
    assert!(
        error.contains(TERRAFORM) && error.contains(FLUX),
        "and both types, got: {error}",
    );

    Ok(())
}
