//! Verifies `forest:render-template@v1` — a *proper* Forest component
//! (lives under `components/forest-contrib/render-template/`, declared
//! via CUE, compiled into the exec-v1 image) runs end-to-end through the
//! exec runner. Companion to the other component tests; this is the
//! first one that follows the canonical Forest component layout.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn render_template_component_walks_directory_tree() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-render-{}", short_token());

    // Step 1 (host run:) — write a small template directory into /work,
    //   including a path component with a `{{var}}` placeholder so we
    //   can prove path-rendering works.
    // Step 2 (forest:render-template@v1) — interpolate src → dest.
    // Step 3 (host run:) — verify file contents and rendered paths.
    let workflow = r#"
package workflow

steps: [
    {
        name: "scaffold-template"
        run: """
            set -eu
            mkdir -p '/work/tpl/src/{{ project }}'
            cat >'/work/tpl/Cargo.toml' <<EOF
            [package]
            name = "{{project}}"
            edition = "2024"
            # Owner: {{ org }}
            EOF
            cat >'/work/tpl/src/{{ project }}/main.rs' <<EOF
            fn main() { println!("hi from {{project}} ({{org}})"); }
            EOF
            cat >'/work/tpl/README.md' <<'EOF'
            literal {{ untouched }} stays as-is in render output? no — it errors.
            EOF
            # Replace README with one that uses only known vars
            cat >'/work/tpl/README.md' <<EOF
            # {{project}}

            Owned by **{{org}}**.
            EOF
            ls -laR /work/tpl
            echo SCAFFOLD_OK
            """
    },
    {
        name: "render"
        uses: "forest:render-template@v1"
        with: {
            src:  "/work/tpl"
            dest: "/work/out"
            vars: {
                project: "hello-svc"
                org:     "forge-rocket"
            }
        }
    },
    {
        name: "verify"
        run: """
            set -eu
            echo "STEP_RENDER_FILES_RENDERED=$STEP_RENDER_FILES_RENDERED"
            test "$STEP_RENDER_FILES_RENDERED" = "3" && echo COUNT_OK

            test -f /work/out/Cargo.toml                && echo HAS_CARGO
            test -f /work/out/README.md                 && echo HAS_README
            test -f /work/out/src/hello-svc/main.rs     && echo HAS_PATH_RENDERED

            grep -q 'name = "hello-svc"'                  /work/out/Cargo.toml && echo CARGO_NAME_OK
            grep -q '# Owner: forge-rocket'               /work/out/Cargo.toml && echo CARGO_OWNER_OK
            grep -F 'Owned by **forge-rocket**'           /work/out/README.md  >/dev/null && echo README_OK
            grep -q 'hi from hello-svc (forge-rocket)'    /work/out/src/hello-svc/main.rs && echo SOURCE_OK

            echo RENDER_TEMPLATE_OK
            """
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "forge-rocket".into(),
            project: "hello-svc".into(),
            release_files: vec![(
                "prod/render-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-render-1".into(),
        release_intent_id: "int-render-1".into(),
        artifact_id: "art-render-1".into(),
        destination_id: "dest-render-1".into(),
        destination: Some(DestinationInfo {
            name: "render-dest".into(),
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
        eprintln!("\n--- render-template stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- render-template stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "render-template workflow failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "SCAFFOLD_OK",
        "COUNT_OK",
        "HAS_CARGO",
        "HAS_README",
        "HAS_PATH_RENDERED",
        "CARGO_NAME_OK",
        "CARGO_OWNER_OK",
        "README_OK",
        "SOURCE_OK",
        "RENDER_TEMPLATE_OK",
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
