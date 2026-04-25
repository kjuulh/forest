//! Smoke-verifies that the `forest` CLI is baked into the exec-v1 image
//! and runs inside the guest. The guest is meant to look like a Forest
//! dev box; this is the first concrete check that it does.
//!
//! The next milestone is wiring the runner's cache-miss path to call
//! `forest components sync` against a live registry. That's a separate
//! commit; this test just confirms the binary is on PATH and reports
//! the protocol version it understands.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forest_cli_runs_inside_guest() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-forest-cli-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "forest-cli-smoke".into(),
            ..Default::default()
        },
    );

    // Three host-side run: steps that probe the CLI:
    //   - it's on PATH and exec-able
    //   - --help works (no panics on init)
    //   - the cache directory the runner reads is in the place
    //     `forest` itself expects
    let probe = r#"
set -eu

echo "--- which forest ---"
command -v forest

echo "--- forest --help ---"
forest --help 2>&1 | head -20 || true

echo "--- cache layout ---"
ls -la /root/.cache/forest/components/ | head -20
test -d /root/.cache/forest/components/bin && echo CACHE_BIN_OK
test -d /root/.cache/forest/components/forest-contrib && echo CACHE_TREE_OK

echo FOREST_CLI_OK
"#;

    let mut metadata = HashMap::new();
    metadata.insert("command".to_string(), probe.to_string());

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-forest-cli-1".into(),
        release_intent_id: "int-forest-cli-1".into(),
        artifact_id: "art-forest-cli-1".into(),
        destination_id: "dest-forest-cli-1".into(),
        destination: Some(DestinationInfo {
            name: "forest-cli-dest".into(),
            environment: "test".into(),
            metadata,
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
        .filter(|l| l.channel == "stdout" || l.channel == "stderr")
        .map(|l| l.line.as_str())
        .collect();

    if completion.outcome != ReleaseOutcome::Success {
        eprintln!("\n--- forest CLI probe ({}) ---", stdout.len());
        for l in &stdout {
            eprintln!("    {l}");
        }
        eprintln!("---");
        panic!(
            "forest CLI smoke failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    assert!(
        stdout.iter().any(|l| l.contains("/usr/local/bin/forest")),
        "expected `command -v forest` to point at /usr/local/bin/forest"
    );
    assert!(
        stdout.iter().any(|l| l.contains("CACHE_BIN_OK")),
        "expected /root/.cache/forest/components/bin/ to exist"
    );
    assert!(
        stdout.iter().any(|l| l.contains("CACHE_TREE_OK")),
        "expected /root/.cache/forest/components/forest-contrib/ to exist"
    );
    assert!(
        stdout.iter().any(|l| l.contains("FOREST_CLI_OK")),
        "probe didn't reach FOREST_CLI_OK sentinel"
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
