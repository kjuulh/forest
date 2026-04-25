//! Step outputs are uniform across both `uses:` backends:
//!
//!   - Native (`<org>/<name>@<version>`): top-level scalar keys of the
//!     component's JSON return value become `STEP_<NAME>_<KEY>` env
//!     vars in subsequent steps.
//!   - Container (`<image>`): the action writes `key=value` lines to
//!     the path in `$FOREST_OUTPUT`, runner promotes those to env vars
//!     after the container exits.
//!
//! The workflow exercises both, then a host-side `run:` step asserts on
//! the resulting env vars by name. The contract from the workflow
//! author's POV is identical regardless of backend — that's the whole
//! point of "feels native".

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn step_outputs_are_uniform_across_backends() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-outputs-{}", short_token());

    let workflow = r#"
package workflow

steps: [
    // 1. Native component — emits {files_written, template, work_dir}
    //    on stdout. The runner promotes the scalar keys to env vars:
    //    STEP_INIT_TEMPLATE, STEP_INIT_WORK_DIR. (files_written is an
    //    array, so it stays in the .json sidecar but isn't promoted.)
    {
        name: "init"
        uses: "forest-contrib/init@0.1.0"
        with: {
            project_name: "outputs-svc"
            organisation: "step-out-org"
        }
    },

    // 2. Container action — alpine writes key=value pairs to
    //    $FOREST_OUTPUT (a file mounted in by the runner). After the
    //    container exits, the runner reads the file and exports
    //    STEP_GATHER_GREETING / STEP_GATHER_BUILD_ID.
    {
        name: "gather"
        uses: "docker.io/library/alpine:3.21"
        run: """
            : "${FOREST_OUTPUT:?missing FOREST_OUTPUT — runner contract broken}"
            {
              echo "greeting=hello-from-container"
              echo "build_id=$(date +%s)"
            } >> "$FOREST_OUTPUT"
            echo "container action wrote outputs"
            """
    },

    // 3. Host-side step — both sources of outputs land in the same
    //    STEP_<NAME>_<KEY> namespace, so the workflow author reads them
    //    the same way regardless of where they came from.
    {
        name: "assert"
        run: """
            set -eu
            echo "STEP_INIT_TEMPLATE=$STEP_INIT_TEMPLATE"
            echo "STEP_INIT_WORK_DIR=$STEP_INIT_WORK_DIR"
            echo "STEP_GATHER_GREETING=$STEP_GATHER_GREETING"
            echo "STEP_GATHER_BUILD_ID=$STEP_GATHER_BUILD_ID"

            test "$STEP_INIT_TEMPLATE"     = "rust-cli"             && echo NATIVE_OUT_OK
            test "$STEP_INIT_WORK_DIR"     = "/work"                && echo NATIVE_CONTEXT_OK
            test "$STEP_GATHER_GREETING"   = "hello-from-container" && echo CONTAINER_OUT_OK

            # build_id is a unix timestamp — assert it parses as a number
            case "$STEP_GATHER_BUILD_ID" in
              ''|*[!0-9]*) echo "build_id not numeric: $STEP_GATHER_BUILD_ID" >&2; exit 1 ;;
              *) echo CONTAINER_DYNAMIC_OK ;;
            esac

            echo OUTPUTS_OK
            """
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "step-out-org".into(),
            project: "outputs-svc".into(),
            release_files: vec![(
                "prod/outputs-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-outputs-1".into(),
        release_intent_id: "int-outputs-1".into(),
        artifact_id: "art-outputs-1".into(),
        destination_id: "dest-outputs-1".into(),
        destination: Some(DestinationInfo {
            name: "outputs-dest".into(),
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
        .wait_for_completion(&release_token, Duration::from_secs(180))
        .await?;

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    if completion.outcome != ReleaseOutcome::Success {
        let stderr: Vec<&str> = logs
            .iter()
            .filter(|l| l.channel == "stderr")
            .map(|l| l.line.as_str())
            .collect();
        eprintln!("\n--- step-outputs stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- step-outputs stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "step-outputs workflow failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "NATIVE_OUT_OK",
        "NATIVE_CONTEXT_OK",
        "CONTAINER_OUT_OK",
        "CONTAINER_DYNAMIC_OK",
        "OUTPUTS_OK",
        "EXEC_WORKFLOW_OK",
    ] {
        assert!(
            stdout.iter().any(|l| l.contains(sentinel)),
            "missing '{sentinel}'. stdout:\n{stdout:#?}"
        );
    }

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
