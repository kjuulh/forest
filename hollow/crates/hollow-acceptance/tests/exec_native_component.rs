//! Verifies the `uses: forest:NAME@VER` shape: the runner resolves to a
//! native binary baked into exec-v1, invokes it via the components-v2
//! SDK protocol, and gets a populated /work back without going through
//! podman. Same workflow shape as the container-action tests so the
//! contract feels uniform across both backends.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exec_runs_native_forest_init_component() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-native-init-{}", short_token());

    // The init component renders its scaffold into context.work_dir
    // (which the runner sets to /work). Step 2 walks the result with
    // host-side shell and asserts every expected file is present with
    // the right interpolated values.
    let workflow = r#"
package workflow

steps: [
    {
        name: "init"
        uses: "forest:init@v1"
        with: {
            project_name: "scaffolded-svc"
            organisation: "forge-rocket"
            license:      "Apache-2.0"
            template:     "rust-cli"
        }
    },
    {
        name: "verify"
        run: """
            set -eu
            echo "--- /work tree ---"
            find /work -type f | sort
            echo "--- assertions ---"

            test -f /work/Cargo.toml      && echo HAS_CARGO_TOML
            test -f /work/src/main.rs     && echo HAS_MAIN_RS
            test -f /work/README.md       && echo HAS_README
            test -f /work/.gitignore      && echo HAS_GITIGNORE
            test -f /work/forest.cue      && echo HAS_FOREST_CUE

            grep -q 'name = "scaffolded-svc"'    /work/Cargo.toml && echo CARGO_HAS_NAME
            grep -q 'license = "Apache-2.0"'      /work/Cargo.toml && echo CARGO_HAS_LICENSE
            grep -q 'forge-rocket'                /work/Cargo.toml && echo CARGO_HAS_ORG

            grep -q 'Hello from scaffolded-svc'   /work/src/main.rs && echo MAIN_RENDERED

            grep -q 'organisation: "forge-rocket"' /work/forest.cue && echo CUE_RENDERED

            echo NATIVE_COMPONENT_OK
            """
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "forge-rocket".into(),
            project: "scaffolded-svc".into(),
            release_files: vec![(
                "prod/native-init-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-native-init-1".into(),
        release_intent_id: "int-native-init-1".into(),
        artifact_id: "art-native-init-1".into(),
        destination_id: "dest-native-init-1".into(),
        destination: Some(DestinationInfo {
            name: "native-init-dest".into(),
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

    // Native components don't pull images, so this should be very fast
    // — but give it an honest budget for cold-cache builds of the
    // exec-v1 image itself.
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
        eprintln!("\n--- native init stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- native init stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "native init failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "HAS_CARGO_TOML",
        "HAS_MAIN_RS",
        "HAS_README",
        "HAS_GITIGNORE",
        "HAS_FOREST_CUE",
        "CARGO_HAS_NAME",
        "CARGO_HAS_LICENSE",
        "CARGO_HAS_ORG",
        "MAIN_RENDERED",
        "CUE_RENDERED",
        "NATIVE_COMPONENT_OK",
        "EXEC_WORKFLOW_OK",
    ] {
        assert!(
            stdout.iter().any(|l| l.contains(sentinel)),
            "missing '{sentinel}'. Full stdout:\n{stdout:#?}"
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
