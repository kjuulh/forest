//! Real-world workflow: create a (proxy for Gitea) remote, clone a
//! template, render with vars, push to the remote. Wires three Forest
//! components in one go through the canonical
//! `<org>/<name>@<version>` cache-resolution path:
//!
//!   forest-contrib/checkout@0.1.0
//!   forest-contrib/render-template@0.1.0
//!   forest-contrib/git-commit-push@0.1.0
//!
//! The "Gitea create" step uses `git init --bare` for a file:// remote
//! since we don't run a Gitea instance in the test loop. The component
//! that actually hits Gitea's API lands separately (Deno-shaped, HTTP-
//! heavy) and gets its own test against a mock server.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_world_create_render_commit_pipeline() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-rwf-{}", short_token());

    // Workflow:
    //   1. host: lay down a "template repo" — Cargo.toml.tmpl + main.rs
    //            + README, all using `{{var}}` placeholders. Init it as
    //            a regular git repo and bare-clone to /work/template-remote.git
    //            so checkout has somewhere to clone from.
    //   2. host: create the "destination remote" — bare repo at
    //            /work/dest-remote.git. This stands in for `forest-contrib/
    //            gitea-create-repo@v0.1` which would call the Gitea API
    //            in production.
    //   3. checkout:        clone template-remote into /work/template
    //   4. render-template: /work/template → /work/out with vars
    //   5. git-commit-push: push /work/out to file:///work/dest-remote.git
    //   6. host: clone dest-remote into /work/verify, assert files landed.
    let workflow = r#"
package workflow

steps: [
    {
        name: "scaffold-template-source"
        run: """
            set -eu
            mkdir -p /work/template-src
            cat >/work/template-src/Cargo.toml <<EOF
            [package]
            name = "{{ project }}"
            edition = "2024"
            # Owner: {{ org }}
            EOF
            cat >/work/template-src/README.md <<EOF
            # {{ project }}

            Owned by {{ org }}.
            EOF
            mkdir -p /work/template-src/src
            cat >/work/template-src/src/main.rs <<EOF
            fn main() { println!("hi from {{project}}"); }
            EOF

            # Push templates into a bare repo so step 'checkout' has a
            # legitimate URL to clone from.
            git init -q -b main /work/template-src
            git -C /work/template-src config user.email ci@forest.local
            git -C /work/template-src config user.name "Forest CI"
            git -C /work/template-src add -A
            git -C /work/template-src commit -q -m "initial template"
            git clone --bare /work/template-src /work/template-remote.git >/dev/null

            echo TEMPLATE_SETUP_OK
            """
    },
    {
        name: "create-destination"
        run: """
            set -eu
            # Stand-in for forest-contrib/gitea-create-repo@v0.1 — that
            # component would create a fresh Gitea repo and emit its
            # clone_url. Here we just make a bare repo at a known path.
            git init -q --bare -b main /work/dest-remote.git
            echo DEST_SETUP_OK=file:///work/dest-remote.git
            """
    },
    {
        name: "checkout"
        uses: "forest-contrib/checkout@0.1.0"
        with: {
            repo:  "file:///work/template-remote.git"
            ref:   "main"
            depth: 1
            dest:  "/work/template"
        }
    },
    {
        name: "render"
        uses: "forest-contrib/render-template@0.1.0"
        with: {
            src:  "/work/template"
            dest: "/work/out"
            vars: {
                project: "scaffolded-svc"
                org:     "forge-rocket"
            }
        }
    },
    {
        name: "publish"
        uses: "forest-contrib/git-commit-push@0.1.0"
        with: {
            repo:       "/work/out"
            remote_url: "file:///work/dest-remote.git"
            branch:     "main"
            message:    "scaffold scaffolded-svc"
            user_name:  "Forest CI"
            user_email: "ci@forest.local"
        }
    },
    {
        name: "verify"
        run: """
            set -eu
            # Clone the destination back out and inspect it: the rendered
            # files must be there with the right interpolated values,
            # and there must be a single commit on `main`.
            git clone -q file:///work/dest-remote.git /work/verify
            cd /work/verify

            test -f Cargo.toml                          && echo HAS_CARGO
            test -f README.md                           && echo HAS_README
            test -f src/main.rs                         && echo HAS_SOURCE

            grep -q 'name = "scaffolded-svc"'    Cargo.toml && echo CARGO_RENDERED
            grep -q '# Owner: forge-rocket'      Cargo.toml && echo OWNER_RENDERED
            grep -q 'hi from scaffolded-svc'     src/main.rs && echo SOURCE_RENDERED
            grep -F  'Owned by forge-rocket.'    README.md  >/dev/null && echo README_RENDERED

            echo "--- verify diagnostics ---"
            git log --oneline
            echo "--- end ---"
            commits=$(git log --oneline | wc -l | tr -d ' ')
            echo "COMMIT_COUNT=$commits"
            if [ "$commits" = "1" ]; then echo SINGLE_COMMIT; fi
            git log -1 --pretty=%s | grep -q 'scaffold scaffolded-svc' && echo COMMIT_MESSAGE_OK

            # Check the components emitted the outputs we expect, the
            # same shape regardless of which component produced them.
            echo "STEP_CHECKOUT_BRANCH=$STEP_CHECKOUT_BRANCH"
            test "$STEP_CHECKOUT_BRANCH" = "main" && echo CHECKOUT_OUTPUT_OK
            echo "STEP_RENDER_FILES_RENDERED=$STEP_RENDER_FILES_RENDERED"
            test "$STEP_RENDER_FILES_RENDERED" = "3" && echo RENDER_OUTPUT_OK
            echo "STEP_PUBLISH_PUSHED_BRANCH=$STEP_PUBLISH_PUSHED_BRANCH"
            test "$STEP_PUBLISH_PUSHED_BRANCH" = "main" && echo PUBLISH_OUTPUT_OK

            echo REAL_WORLD_OK
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
                "prod/rwf-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-rwf-1".into(),
        release_intent_id: "int-rwf-1".into(),
        artifact_id: "art-rwf-1".into(),
        destination_id: "dest-rwf-1".into(),
        destination: Some(DestinationInfo {
            name: "rwf-dest".into(),
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
        eprintln!("\n--- real-world stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- real-world stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "real-world workflow failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "TEMPLATE_SETUP_OK",
        "DEST_SETUP_OK=file:///work/dest-remote.git",
        "HAS_CARGO",
        "HAS_README",
        "HAS_SOURCE",
        "CARGO_RENDERED",
        "OWNER_RENDERED",
        "SOURCE_RENDERED",
        "README_RENDERED",
        "SINGLE_COMMIT",
        "COMMIT_MESSAGE_OK",
        "CHECKOUT_OUTPUT_OK",
        "RENDER_OUTPUT_OK",
        "PUBLISH_OUTPUT_OK",
        "REAL_WORLD_OK",
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
