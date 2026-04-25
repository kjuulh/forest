//! Verifies podman runs end-to-end inside the exec-v1 Firecracker guest.
//!
//! `podman info` exercises storage init (overlay over tmpfs graphroot) and
//! networking init (netavark + aardvark-dns). `podman run --rm alpine echo`
//! exercises the full pull → unpack → exec → cleanup cycle against a real
//! registry, which is the exact path the workflow runner's `uses:` mode
//! relies on.
//!
//! Run via `metadata.command` override on the existing exec/v1 destination
//! so we don't have to land the v1 runner before knowing whether podman
//! actually works in this kernel.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn podman_runs_a_container_in_the_guest() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/exec/1").await?;

    let release_token = format!("tkn-podman-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "podman-substrate".into(),
            ..Default::default()
        },
    );

    // Smoke script. Each gate prints a `PODMAN_*` sentinel that the test
    // greps for; failures dump podman's own stderr verbatim so we can see
    // exactly which step the runtime tripped on.
    let probe = r#"
echo PODMAN_BEGIN

if ! podman version 2>&1; then
  echo PODMAN_VERSION_FAILED
  exit 2
fi
echo PODMAN_VERSION_OK

# `podman info` is the heaviest init: storage driver probe, network stack
# bring-up, registries config parse. If this works the runtime is alive.
if ! podman info 2>&1; then
  echo PODMAN_INFO_FAILED
  exit 3
fi
echo PODMAN_INFO_OK

# Pull + run a tiny image. Pull and run are split so podman's progress
# bars (which use \r to overwrite the same line) don't garble the captured
# stdout from `echo HELLO_FROM_CONTAINER`.
#
# `--network=host` skips netavark/aardvark and shares the VM's network
# stack with the container. The VM is already the isolation boundary,
# so per-container network namespacing buys nothing here and avoids
# the iptables-nft-vs-legacy compatibility dance.
if ! podman pull --quiet public.ecr.aws/docker/library/alpine:3.21 >/tmp/pull.log 2>&1; then
  echo "PODMAN_PULL_FAILED:"
  cat /tmp/pull.log
  exit 5
fi
echo PODMAN_PULL_OK

if ! out=$(podman run --rm --network=host public.ecr.aws/docker/library/alpine:3.21 echo HELLO_FROM_CONTAINER 2>&1); then
  echo "PODMAN_RUN_FAILED: $out"
  exit 4
fi
echo "PODMAN_RUN_OUTPUT=$out"

echo PODMAN_END
"#;

    let mut metadata = HashMap::new();
    metadata.insert("command".to_string(), probe.to_string());

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-podman-1".into(),
        release_intent_id: "int-podman-1".into(),
        artifact_id: "art-podman-1".into(),
        destination_id: "dest-podman-1".into(),
        destination: Some(DestinationInfo {
            name: "podman-probe-dest".into(),
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
        .wait_for_completion(&release_token, Duration::from_secs(180))
        .await?;

    let lines: Vec<String> = logs
        .iter()
        .filter(|l| l.channel == "stdout" || l.channel == "stderr")
        .map(|l| l.line.clone())
        .collect();

    eprintln!("\n--- podman probe output ({} lines) ---", lines.len());
    for line in &lines {
        eprintln!("    {line}");
    }
    eprintln!("---");

    if completion.outcome != ReleaseOutcome::Success {
        panic!(
            "podman probe failed: {:?} {}",
            completion.outcome, completion.error_message
        );
    }

    assert!(
        lines.iter().any(|l| l == "PODMAN_VERSION_OK"),
        "`podman version` failed inside the guest"
    );
    assert!(
        lines.iter().any(|l| l == "PODMAN_INFO_OK"),
        "`podman info` failed inside the guest — storage/network init broken"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("PODMAN_RUN_OUTPUT=HELLO_FROM_CONTAINER")),
        "`podman run alpine echo` didn't produce expected output"
    );
    assert!(
        lines.iter().any(|l| l == "PODMAN_END"),
        "probe didn't reach PODMAN_END"
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
