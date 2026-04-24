//! Orchestrator-level acceptance test: drives a release through the full
//! controller → agent → Firecracker → guest pipeline, bypassing only the real
//! forest-server (replaced by the in-process [`FakeServer`] puppet).
//!
//! The test:
//!   1. Boots the orchestrator (fake server + controller locally + agent
//!      remote via reverse-tunnelled SSH).
//!   2. Installs a canned release fixture on the puppet.
//!   3. Puppets a `WorkAssignment` with an `echo` destination.
//!   4. Asserts the puppet received the expected log line and a SUCCESS
//!      completion.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn echo_through_orchestrator() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/echo/1").await?;

    let release_token = format!("tkn-{}", uuid::new_v4_short());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "orchestrator-test".into(),
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-orch-1".into(),
        release_intent_id: "int-orch-1".into(),
        artifact_id: "art-orch-1".into(),
        destination_id: "dest-orch-1".into(),
        destination: Some(DestinationInfo {
            name: "orchestrator-test-dest".into(),
            environment: "test".into(),
            metadata: HashMap::from([("command".into(), "echo through-orchestrator".into())]),
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "echo".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        mode: ReleaseMode::Deploy.into(),
    };

    orchestrator.fake_server.dispatch(assignment)?;

    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(90))
        .await?;

    assert_eq!(
        completion.outcome,
        ReleaseOutcome::Success,
        "expected SUCCESS, got {completion:?}; logs: {logs:#?}"
    );
    assert!(
        logs.iter()
            .any(|l| l.line.contains("through-orchestrator")),
        "expected log line with 'through-orchestrator', got: {logs:#?}"
    );

    // Guest serial console should also have been forwarded over gRPC
    // (agent → controller → PushLogs → fake forest-server) with
    // channel="console", proving boot-level diagnostics are visible in the
    // real release-log path.
    let console_lines: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "console")
        .map(|l| l.line.as_str())
        .collect();
    assert!(
        console_lines.iter().any(|l| l.contains("Linux version")),
        "kernel banner missing from forwarded console logs; got {} console lines",
        console_lines.len()
    );

    Ok(())
}

mod uuid {
    // The test harness already depends on `uuid` transitively; avoid pulling
    // it into hollow-acceptance's dev-deps by generating a short random token
    // here.
    pub fn new_v4_short() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{nanos:x}")
    }
}
