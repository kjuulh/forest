//! Verifies `forest-contrib/gitea-create-repo@0.1.0` against a tiny
//! socat-based HTTP mock running inside the same VM. The mock accepts a
//! single POST and emits a canned Gitea-shaped 201 response with the
//! fields the component reads (id, clone_url, ssh_url, html_url,
//! full_name).
//!
//! Real-Gitea integration belongs in its own dedicated test (boot a
//! Gitea container as step 0, do programmatic admin user setup, hit
//! the live API). That's heavier than this unit-style verification
//! warrants — what we want here is to prove the component actually
//! wires up its inputs, hits the right URL, parses the response, and
//! emits the right outputs.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gitea_create_repo_against_mock_server() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-gitea-{}", short_token());

    let workflow = r#"
package workflow

steps: [
    // 1. Drop the API token onto disk where the component will read it
    //    from. Production wiring uses Forest's secret channel, but a
    //    plain `run:` step is sufficient for the unit-style mock test.
    {
        name: "seed-token"
        run: """
            mkdir -p /run/secrets
            printf 'mock-token-1234' > /run/secrets/gitea-token
            chmod 0600 /run/secrets/gitea-token
            echo TOKEN_OK
            """
    },

    // 2. Spin up the mock Gitea API on 127.0.0.1:3000. socat in
    //    fork-listen mode hands each connection to a short shell that
    //    drains the HTTP request and replies with a canned 201 Created.
    //    Headers use explicit \r\n + Content-Length so ureq's parser
    //    accepts the response cleanly.
    {
        name: "start-mock"
        run: """
            cat >/tmp/gitea-response.sh <<'OUTER'
            #!/bin/sh
            # Drain the HTTP request — read headers until the empty
            # CRLF terminator. We don't bother parsing the body since
            # the test doesn't care what we POSTed.
            while IFS= read -r line; do
              line=${line%$(printf '\r')}
              [ -z "$line" ] && break
            done

            BODY='{"id":4242,"full_name":"forge-rocket/scaffolded-svc","clone_url":"http://gitea.local/forge-rocket/scaffolded-svc.git","ssh_url":"git@gitea.local:forge-rocket/scaffolded-svc.git","html_url":"http://gitea.local/forge-rocket/scaffolded-svc"}'
            LEN=${#BODY}
            printf 'HTTP/1.1 201 Created\\r\\n'
            printf 'Content-Type: application/json\\r\\n'
            printf 'Content-Length: %d\\r\\n' "$LEN"
            printf 'Connection: close\\r\\n'
            printf '\\r\\n'
            printf '%s' "$BODY"
            OUTER
            chmod +x /tmp/gitea-response.sh

            (socat -t 2 TCP-LISTEN:3000,fork,reuseaddr EXEC:/tmp/gitea-response.sh >/tmp/socat.log 2>&1) &
            echo $! >/tmp/socat.pid

            # Wait for the listener to bind. /dev/tcp isn't available in
            # busybox sh, so use socat's own client mode to probe.
            for i in 1 2 3 4 5 6 7 8 9 10; do
              if socat -T 1 -u TCP:127.0.0.1:3000 - </dev/null >/dev/null 2>&1; then
                echo MOCK_READY
                break
              fi
              sleep 0.2
            done
            """
    },

    // 3. Run the component against the mock.
    {
        name: "create"
        uses: "forest-contrib/gitea-create-repo@0.1.0"
        with: {
            base_url:   "http://127.0.0.1:3000"
            org:        "forge-rocket"
            name:       "scaffolded-svc"
            description: "scaffolded by forest workflow"
            private:    true
            auto_init:  false
            token_path: "/run/secrets/gitea-token"
        }
    },

    // 4. Assert the component parsed the mock response and surfaced it
    //    through the standard STEP_<NAME>_<KEY> output channel.
    {
        name: "verify"
        run: """
            set -eu
            kill "$(cat /tmp/socat.pid)" 2>/dev/null || true

            echo "STEP_CREATE_ID=$STEP_CREATE_ID"
            echo "STEP_CREATE_CLONE_URL=$STEP_CREATE_CLONE_URL"
            echo "STEP_CREATE_SSH_URL=$STEP_CREATE_SSH_URL"
            echo "STEP_CREATE_HTML_URL=$STEP_CREATE_HTML_URL"
            echo "STEP_CREATE_FULL_NAME=$STEP_CREATE_FULL_NAME"

            test "$STEP_CREATE_ID"        = "4242"                                             && echo ID_OK
            test "$STEP_CREATE_FULL_NAME" = "forge-rocket/scaffolded-svc"                      && echo FULL_NAME_OK
            test "$STEP_CREATE_CLONE_URL" = "http://gitea.local/forge-rocket/scaffolded-svc.git" && echo CLONE_OK
            test "$STEP_CREATE_SSH_URL"   = "git@gitea.local:forge-rocket/scaffolded-svc.git"  && echo SSH_OK
            test "$STEP_CREATE_HTML_URL"  = "http://gitea.local/forge-rocket/scaffolded-svc"   && echo HTML_OK

            echo GITEA_CREATE_OK
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
                "prod/gitea-dest/forest/exec@1/workflow.cue".to_string(),
                workflow.to_string(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-gitea-1".into(),
        release_intent_id: "int-gitea-1".into(),
        artifact_id: "art-gitea-1".into(),
        destination_id: "dest-gitea-1".into(),
        destination: Some(DestinationInfo {
            name: "gitea-dest".into(),
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
        eprintln!("\n--- gitea stdout ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("--- gitea stderr ({}) ---", stderr.len());
        for l in &stderr {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "gitea workflow failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    for sentinel in &[
        "TOKEN_OK",
        "MOCK_READY",
        "ID_OK",
        "FULL_NAME_OK",
        "CLONE_OK",
        "SSH_OK",
        "HTML_OK",
        "GITEA_CREATE_OK",
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
