//! Realistic GitHub-Actions-shaped CI pipeline: scaffold → tree → lint →
//! test → package → verify → git snapshot, using three different raw OCI
//! images (alpine:3.21, python:3.12-alpine, alpine/git:latest) with
//! `/work` shared across every step.
//!
//! This is the substrate-stress complement to `pipeline_stress` — that
//! test uses one image (the exec-v1 host) for everything; this one
//! exercises multi-image pulls, podman storage churn, and the
//! `uses + run` flexible shape that maps to GHA's container actions.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_ci_multi_image_workflow() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-ci-{}", short_token());

    // The CUE manifest references three images, exercises CUE's hidden
    // fields (`_`-prefixed) for shared values, and mixes plain `run:`
    // host-side steps with `uses + run` container-action steps.
    //
    // Storage budget inside the guest:
    //   /var/tmp tmpfs = 256 MiB (image cache lives here)
    //   alpine:3.21      ~5 MiB compressed
    //   alpine/git       ~10 MiB compressed (alpine + git)
    //   python:3.12-alpine ~50 MiB compressed (~150 MiB unpacked)
    // Total well under the cap.
    let workflow = r#"
package workflow

_image_alpine: "docker.io/library/alpine:3.21"
_image_python: "docker.io/library/python:3.12-alpine"
_image_git:    "docker.io/alpine/git:latest"

steps: [
    // 1. Scaffold (host-side) — mimics actions/checkout dropping a repo
    //    onto the workspace. We generate the source so the test is
    //    deterministic and offline-tolerant.
    {
        name: "scaffold"
        run: """
            set -eu
            mkdir -p /work/src /work/tests
            cat >/work/src/__init__.py <<EOF
            EOF
            cat >/work/src/hello.py <<EOF
            def greet(name: str) -> str:
                return f"Hello, {name}!"
            EOF
            cat >/work/tests/__init__.py <<EOF
            EOF
            cat >/work/tests/test_hello.py <<EOF
            import unittest
            from src.hello import greet

            class TestHello(unittest.TestCase):
                def test_basic(self):
                    self.assertEqual(greet("forest"), "Hello, forest!")

                def test_empty(self):
                    self.assertEqual(greet(""), "Hello, !")

            if __name__ == "__main__":
                unittest.main()
            EOF
            echo SCAFFOLD_OK
            """
    },

    // 2. Tree dump in alpine — host VM has its own bash; this exercises
    //    `uses + run` with the smallest possible image.
    {
        name: "tree"
        uses: _image_alpine
        run: """
            cd /work
            find . -type f | sort | head -20
            echo TREE_OK
            """
    },

    // 3. Lint via Python's built-in py_compile. No pip install needed.
    {
        name: "lint"
        uses: _image_python
        run: """
            cd /work
            python -m compileall -q src/ tests/
            echo LINT_OK
            """
    },

    // 4. Test via Python's built-in unittest. Same image — kernel
    //    overlay should layer-cache the previous pull.
    {
        name: "test"
        uses: _image_python
        run: """
            cd /work
            python -m unittest tests.test_hello -v 2>&1
            echo TEST_OK
            """
    },

    // 5. Package the artifact (back in alpine, which is already pulled).
    {
        name: "package"
        uses: _image_alpine
        run: """
            cd /work
            tar czf release.tar.gz src/
            sha256sum release.tar.gz | tee release.sha256
            ls -la release.*
            echo PACKAGE_OK
            """
    },

    // 6. Verify the artifact's checksum (in a fresh container — proves
    //    /work persists across container boundaries).
    {
        name: "verify"
        uses: _image_alpine
        run: """
            cd /work
            sha256sum -c release.sha256
            echo VERIFY_OK
            """
    },

    // 7. Git snapshot via alpine/git — uses+run with --entrypoint=/bin/sh
    //    overrides alpine/git's `git` ENTRYPOINT so we can run a script.
    {
        name: "git-snapshot"
        uses: _image_git
        env: {
            GIT_AUTHOR_NAME:     "Forest CI"
            GIT_AUTHOR_EMAIL:    "ci@forest.local"
            GIT_COMMITTER_NAME:  "Forest CI"
            GIT_COMMITTER_EMAIL: "ci@forest.local"
        }
        run: """
            cd /work
            git init -q -b main .
            git add -A
            git commit -q -m "ci: build artifact"
            git log --oneline | head
            echo SNAPSHOT_OK
            """
    },

    // 8. Final cross-tool assert (host-side).
    {
        name: "assert"
        run: """
            set -eu
            test -f /work/release.tar.gz && echo HAS_TARBALL
            test -f /work/release.sha256 && echo HAS_CHECKSUM
            test -d /work/.git           && echo HAS_REPO
            echo CI_PIPELINE_OK
            """
    },
]
"#;

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "ci-pipeline".into(),
            release_files: vec![(
                "prod/ci-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-ci-1".into(),
        release_intent_id: "int-ci-1".into(),
        artifact_id: "art-ci-1".into(),
        destination_id: "dest-ci-1".into(),
        destination: Some(DestinationInfo {
            name: "ci-dest".into(),
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

    // Three image pulls + multi-step container churn — give it 6 minutes.
    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(360))
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
        eprintln!("\n--- ci pipeline stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- ci pipeline stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "ci pipeline failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "SCAFFOLD_OK",
        "TREE_OK",
        "LINT_OK",
        "TEST_OK",
        "PACKAGE_OK",
        "VERIFY_OK",
        "SNAPSHOT_OK",
        "HAS_TARBALL",
        "HAS_CHECKSUM",
        "HAS_REPO",
        "CI_PIPELINE_OK",
        "EXEC_WORKFLOW_OK",
    ] {
        assert!(
            stdout.iter().any(|l| l.contains(sentinel)),
            "missing '{sentinel}' in stdout. Full stdout:\n{stdout:#?}"
        );
    }

    // Assert the unittest output really executed our tests, not just
    // exited 0. unittest sometimes streams to stderr; merge channels.
    let combined: Vec<&str> = stdout.iter().chain(stderr.iter()).copied().collect();
    assert!(
        combined
            .iter()
            .any(|l| l.contains("test_basic") && l.contains("ok")),
        "unittest output should show test_basic ... ok"
    );
    assert!(
        combined
            .iter()
            .any(|l| l.contains("test_empty") && l.contains("ok")),
        "unittest output should show test_empty ... ok"
    );
    assert!(
        combined.iter().any(|l| l.contains("Ran 2 tests")),
        "unittest summary should report Ran 2 tests"
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
