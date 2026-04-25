//! Acceptance for the `forest/exec/1` destination — Forest's general-purpose
//! CUE-driven workflow runner. Ships a workflow.cue with three `run:` steps,
//! verifies the runner walks them in order, that `env:` mappings reach the
//! step's process, and that a non-zero step short-circuits the workflow.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_runs_cue_workflow_in_order() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-exec-{}", short_token());

    let workflow = r#"
package workflow

steps: [
    {
        name: "first"
        run:  "echo STEP_FIRST_RAN"
    },
    {
        name: "uses-env"
        run:  "echo STEP_ENV_$GREETING"
        env: {
            GREETING: "HELLO"
        }
    },
    {
        name: "writes-file"
        run:  "echo body >/tmp/written && cat /tmp/written | sed 's/^/STEP_FILE_/'"
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "exec-smoke".into(),
            release_files: vec![(
                "prod/exec-smoke-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-exec-1".into(),
        release_intent_id: "int-exec-1".into(),
        artifact_id: "art-exec-1".into(),
        destination_id: "dest-exec-1".into(),
        destination: Some(DestinationInfo {
            name: "exec-smoke-dest".into(),
            environment: "prod".into(),
            metadata: HashMap::new(),
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "exec".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        mode: ReleaseMode::Deploy.into(),
        artifact_store: None,
    };

    orchestrator.fake_server.dispatch(assignment)?;

    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(120))
        .await?;

    if completion.outcome != ReleaseOutcome::Success {
        let relevant: Vec<_> = logs
            .iter()
            .filter(|l| l.channel != "console")
            .take(80)
            .collect();
        panic!(
            "expected SUCCESS, got {:?}: {}\nlogs (non-console, first 80):\n{:#?}",
            completion.outcome, completion.error_message, relevant
        );
    }

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    // Each step prints a unique sentinel; presence proves the step ran.
    assert!(
        stdout.iter().any(|l| l.contains("STEP_FIRST_RAN")),
        "step 'first' didn't run. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("STEP_ENV_HELLO")),
        "step 'uses-env' env merge didn't reach the process. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("STEP_FILE_body")),
        "step 'writes-file' didn't complete its pipeline. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("EXEC_WORKFLOW_OK")),
        "missing EXEC_WORKFLOW_OK sentinel — runner didn't reach the end. \
         stdout:\n{stdout:#?}"
    );

    // Order check: first must precede uses-env which must precede writes-file.
    let pos_first = stdout
        .iter()
        .position(|l| l.contains("STEP_FIRST_RAN"))
        .expect("found above");
    let pos_env = stdout
        .iter()
        .position(|l| l.contains("STEP_ENV_HELLO"))
        .expect("found above");
    let pos_file = stdout
        .iter()
        .position(|l| l.contains("STEP_FILE_body"))
        .expect("found above");
    assert!(
        pos_first < pos_env && pos_env < pos_file,
        "steps ran out of order: first={pos_first} env={pos_env} file={pos_file}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_failing_step_short_circuits() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-exec-fail-{}", short_token());

    let workflow = r#"
package workflow

steps: [
    {
        name: "ok"
        run:  "echo STEP_OK_RAN"
    },
    {
        name: "boom"
        run:  "echo STEP_BOOM_RAN; exit 17"
    },
    {
        name: "should-not-run"
        run:  "echo STEP_LATER_RAN"
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "exec-failure".into(),
            release_files: vec![(
                "prod/exec-fail-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-exec-fail-1".into(),
        release_intent_id: "int-exec-fail-1".into(),
        artifact_id: "art-exec-fail-1".into(),
        destination_id: "dest-exec-fail-1".into(),
        destination: Some(DestinationInfo {
            name: "exec-fail-dest".into(),
            environment: "prod".into(),
            metadata: HashMap::new(),
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "exec".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        mode: ReleaseMode::Deploy.into(),
        artifact_store: None,
    };

    orchestrator.fake_server.dispatch(assignment)?;

    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(120))
        .await?;

    assert_eq!(
        completion.outcome,
        ReleaseOutcome::Failure,
        "expected FAILURE for short-circuit. error: {}",
        completion.error_message
    );

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    assert!(
        stdout.iter().any(|l| l.contains("STEP_OK_RAN")),
        "first step should have run. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("STEP_BOOM_RAN")),
        "failing step should have started. stdout:\n{stdout:#?}"
    );
    assert!(
        !stdout.iter().any(|l| l.contains("STEP_LATER_RAN")),
        "step after failing one should NOT have run (short-circuit broken). stdout:\n{stdout:#?}"
    );
    assert!(
        !stdout.iter().any(|l| l.contains("EXEC_WORKFLOW_OK")),
        "EXEC_WORKFLOW_OK sentinel should be absent on failure. stdout:\n{stdout:#?}"
    );

    Ok(())
}

fn short_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}
