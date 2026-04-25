//! Verifies the script-component path: `uses: forest:git-init@v1` resolves
//! to the script-engine wrapper, which dispatches to a shell script under
//! `/usr/local/lib/forest-components/git-init/v1/scripts/init.sh`. From
//! the workflow author's POV the experience is identical to the compiled
//! `forest:init@v1` component or to a `uses: <image>` container action.
//!
//! The point: shipping a new component is "drop a directory of shell
//! scripts" — no Rust crate, no recompile of the image's binaries.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn script_component_git_init_runs_through_engine() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-script-init-{}", short_token());

    let workflow = r#"
package workflow

steps: [
    // 1. Native compiled component scaffolds files into /work.
    {
        name: "scaffold"
        uses: "forest:init@v1"
        with: {
            project_name: "git-init-svc"
            organisation: "engine-org"
        }
    },

    // 2. Script-component (engine-driven) initialises the git repo.
    //    Outputs branch + initial_commit_sha for step 3 to consume.
    {
        name: "git_setup"
        uses: "forest:git-init@v1"
        with: {
            branch:     "trunk"
            user_email: "ci@forest.local"
            user_name:  "Forest CI"
            message:    "scaffold via forest:init@v1"
        }
    },

    // 3. Host-side assert — proves both kinds of component spoke the
    //    same protocol, /work persisted across them, and the
    //    script-component's outputs flowed through the same
    //    STEP_<NAME>_<KEY> shape as a compiled component.
    {
        name: "assert"
        run: """
            set -eu
            cd /work
            test -d .git                    && echo HAS_GIT_REPO
            test -f Cargo.toml              && echo HAS_SCAFFOLD

            git_branch=$(git rev-parse --abbrev-ref HEAD)
            test "$git_branch" = "trunk"    && echo BRANCH_FROM_INPUT_OK

            log_msg=$(git log -1 --pretty=%B | head -1)
            test "$log_msg" = "scaffold via forest:init@v1" && echo MESSAGE_OK

            # Outputs from the script-component land in the same
            # namespace as compiled components.
            echo "STEP_GIT_SETUP_BRANCH=$STEP_GIT_SETUP_BRANCH"
            echo "STEP_GIT_SETUP_INITIAL_COMMIT_SHA=$STEP_GIT_SETUP_INITIAL_COMMIT_SHA"
            test "$STEP_GIT_SETUP_BRANCH" = "trunk" && echo OUTPUT_BRANCH_OK

            git_sha=$(git rev-parse HEAD)
            test "$git_sha" = "$STEP_GIT_SETUP_INITIAL_COMMIT_SHA" && echo OUTPUT_SHA_OK

            test "$STEP_GIT_SETUP_ALREADY_INITIALIZED" = "false" && echo NEW_REPO_OK

            echo SCRIPT_COMPONENT_OK
            """
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "engine-org".into(),
            project: "git-init-svc".into(),
            release_files: vec![(
                "prod/script-init-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-script-init-1".into(),
        release_intent_id: "int-script-init-1".into(),
        artifact_id: "art-script-init-1".into(),
        destination_id: "dest-script-init-1".into(),
        destination: Some(DestinationInfo {
            name: "script-init-dest".into(),
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
        eprintln!("\n--- script-component stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- script-component stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "script-component workflow failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "HAS_GIT_REPO",
        "HAS_SCAFFOLD",
        "BRANCH_FROM_INPUT_OK",
        "MESSAGE_OK",
        "OUTPUT_BRANCH_OK",
        "OUTPUT_SHA_OK",
        "NEW_REPO_OK",
        "SCRIPT_COMPONENT_OK",
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
