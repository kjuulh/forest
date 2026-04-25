//! Stress-test pipeline that mirrors a realistic "initialize a service
//! project" workflow inside the exec/v1 substrate. Hits everything we
//! have today and surfaces whatever the limits are:
//!
//!   1. CUE evaluation with shared `_vars` interpolated into multiple
//!      steps (proves the manifest format scales beyond toy cases).
//!   2. Scaffolding: writes a Dockerfile + a tiny shell "service" + a
//!      README into /work.
//!   3. git init + add + commit, all inside the VM.
//!   4. `podman build` of the scaffolded Dockerfile (kernel overlay over
//!      tmpfs graphroot, multi-layer image creation).
//!   5. `podman inspect` to read back image metadata.
//!   6. `podman run` of the built image, capturing stdout.
//!   7. Cross-tool verify: git log, podman images, /work files all
//!      consistent.
//!   8. `podman rmi` cleanup.
//!
//! If something falls over (storage, memory, missing kernel feature),
//! the failure points exactly at which step. Worth re-running on every
//! exec-v1 image bump.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_stress_scaffold_build_run_cleanup() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-stress-{}", short_token());

    // CUE manifest. `_`-prefixed fields are hidden — they don't appear in
    // `cue export`'s JSON output but ARE in scope for string interpolation
    // throughout the file. That's how we keep the shared values in one
    // place without leaking them into the runner's step list.
    let workflow = r#"
package workflow

_project_name: "forge-hello"
_org_name:     "forest-test"
// podman auto-prefixes unqualified tags with `localhost/`, so we use that
// prefix explicitly to match — otherwise `podman inspect` and `podman run`
// fail with "no such object" even after a successful tag.
_image_tag:    "localhost/\(_project_name):latest"
_branch:       "main"

steps: [
    {
        name: "scaffold"
        run: """
            set -eu
            mkdir -p /work/src
            cat >/work/src/hello.sh <<EOF
            #!/bin/sh
            echo "Hello from $PROJECT_NAME ($ORG_NAME)"
            EOF
            chmod +x /work/src/hello.sh
            cat >/work/Dockerfile <<EOF
            FROM docker.io/library/alpine:3.21
            COPY src/hello.sh /usr/local/bin/hello
            RUN chmod +x /usr/local/bin/hello
            CMD ["hello"]
            EOF
            cat >/work/README.md <<EOF
            # $PROJECT_NAME

            Owned by: $ORG_NAME
            EOF
            ls -la /work
            echo SCAFFOLD_OK
            """
        env: {
            PROJECT_NAME: _project_name
            ORG_NAME:     _org_name
        }
    },
    {
        name: "git-init"
        run: """
            set -eu
            cd /work
            git init -q -b $BRANCH .
            git config user.email "ci@forest.local"
            git config user.name  "Forest CI"
            git add -A
            git commit -q -m "initial scaffold"
            git log --oneline
            echo GIT_OK
            """
        env: {
            BRANCH: _branch
        }
    },
    {
        name: "build-image"
        run: """
            set -eu
            podman build --network=host -q -t "$IMAGE_TAG" /work
            echo BUILD_OK
            """
        env: {
            IMAGE_TAG: _image_tag
        }
    },
    {
        name: "inspect-image"
        run: """
            set -eu
            cmd=$(podman inspect $IMAGE_TAG --format='{{.Config.Cmd}}')
            echo "INSPECT_CMD=$cmd"
            arch=$(podman inspect $IMAGE_TAG --format='{{.Architecture}}')
            echo "INSPECT_ARCH=$arch"
            layers=$(podman inspect $IMAGE_TAG --format='{{len .RootFS.Layers}}')
            echo "INSPECT_LAYERS=$layers"
            """
        env: {
            IMAGE_TAG: _image_tag
        }
    },
    {
        name: "run-image"
        run: """
            set -eu
            out=$(podman run --rm --network=host $IMAGE_TAG)
            echo "RUN_OUTPUT=$out"
            """
        env: {
            IMAGE_TAG: _image_tag
        }
    },
    {
        name: "verify"
        run: """
            set -eu
            test -d /work/.git && echo HAS_GIT
            git -C /work log --oneline | grep -q "initial scaffold" && echo HAS_COMMIT
            podman images --format '{{.Repository}}:{{.Tag}}' | grep -q "$IMAGE_TAG" && echo HAS_IMAGE
            test -f /work/Dockerfile && echo HAS_DOCKERFILE
            test -x /work/src/hello.sh && echo HAS_SHELL_SCRIPT
            echo VERIFY_OK
            """
        env: {
            IMAGE_TAG: _image_tag
        }
    },
    {
        name: "cleanup"
        run: """
            set -eu
            podman rmi -f $IMAGE_TAG >/dev/null
            # Confirm the rmi actually removed it.
            if podman images --format '{{.Repository}}:{{.Tag}}' | grep -q "$IMAGE_TAG"; then
                echo "CLEANUP_FAILED: image still present" >&2
                exit 1
            fi
            echo CLEANUP_OK
            """
        env: {
            IMAGE_TAG: _image_tag
        }
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "pipeline-stress".into(),
            release_files: vec![(
                "prod/stress-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-stress-1".into(),
        release_intent_id: "int-stress-1".into(),
        artifact_id: "art-stress-1".into(),
        destination_id: "dest-stress-1".into(),
        destination: Some(DestinationInfo {
            name: "stress-dest".into(),
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

    // Pipeline involves a real registry pull + multi-layer build +
    // multiple inspects + cleanup. 5 minutes should be ample even on
    // a slow network or with cold image cache.
    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(300))
        .await?;

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();
    let stderr: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stderr")
        .map(|l| l.line.as_str())
        .collect();

    if completion.outcome != ReleaseOutcome::Success {
        eprintln!("\n--- stress pipeline stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- stress pipeline stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "pipeline failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    // Each step's sentinel proves it completed. Order doesn't need to
    // be enforced explicitly — the runner's fail-fast already does that;
    // a missing earlier sentinel would have surfaced as outcome=Failure.
    for sentinel in &[
        "SCAFFOLD_OK",
        "GIT_OK",
        "BUILD_OK",
        "RUN_OUTPUT=Hello from forge-hello (forest-test)",
        "HAS_GIT",
        "HAS_COMMIT",
        "HAS_IMAGE",
        "HAS_DOCKERFILE",
        "HAS_SHELL_SCRIPT",
        "VERIFY_OK",
        "CLEANUP_OK",
        "EXEC_WORKFLOW_OK",
    ] {
        assert!(
            stdout.iter().any(|l| l.contains(sentinel)),
            "missing '{sentinel}' in stdout. Full stdout:\n{stdout:#?}"
        );
    }

    // Inspect output should have produced reasonable values, not empty.
    let inspect_cmd = stdout
        .iter()
        .find_map(|l| l.strip_prefix("INSPECT_CMD="))
        .expect("INSPECT_CMD line missing");
    assert!(
        inspect_cmd.contains("hello"),
        "image CMD should reference our hello binary, got {inspect_cmd:?}"
    );
    let layers = stdout
        .iter()
        .find_map(|l| l.strip_prefix("INSPECT_LAYERS="))
        .expect("INSPECT_LAYERS line missing");
    let layers_n: u32 = layers.trim().parse().unwrap_or(0);
    assert!(
        layers_n >= 2,
        "expected at least 2 image layers (alpine base + our COPY/RUN), got {layers_n}"
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
