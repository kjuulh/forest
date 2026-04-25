//! Phase-2 fluxv1 acceptance:
//!  1. Toolchain check — image substrate (git + flux CLI + kustomize CLI)
//!     reachable inside the VM, exercised by overriding metadata.command.
//!  2. Real deploy workflow — `/usr/local/bin/forest-flux-deploy` clones a
//!     `file://` bare repo (auto-init), writes manifests at the layout the
//!     legacy fluxv1 destination uses, commits, pushes. Asserts the
//!     `FLUX_PUSHED` sentinel arrived.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fluxv1_toolchain_through_orchestrator() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/fluxv1/1").await?;

    let release_token = format!("tkn-flux-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "fluxv1-smoke".into(),
            ..Default::default()
        },
    );

    let mut metadata = HashMap::new();
    // Override the deploy script with a toolchain-only check; we only
    // need to prove the image substrate is intact here. The real deploy
    // path is exercised by `fluxv1_real_deploy_through_orchestrator`.
    metadata.insert(
        "command".to_string(),
        "git --version && flux --version && kustomize version && echo FLUXV1_TOOLCHAIN_OK"
            .to_string(),
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-flux-1".into(),
        release_intent_id: "int-flux-1".into(),
        artifact_id: "art-flux-1".into(),
        destination_id: "dest-flux-1".into(),
        destination: Some(DestinationInfo {
            name: "fluxv1-smoke-dest".into(),
            environment: "test".into(),
            metadata,
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "fluxv1".into(),
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

    if completion.outcome != ReleaseOutcome::Success {
        let relevant: Vec<_> = logs
            .iter()
            .filter(|l| l.channel != "console")
            .take(40)
            .collect();
        panic!(
            "expected SUCCESS, got {:?}: {}\nlogs (non-console, first 40):\n{:#?}",
            completion.outcome, completion.error_message, relevant
        );
    }

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    // The dispatcher's default fluxv1 command runs the toolchain version
    // checks and a sentinel echo. If the image is correct, all four pieces
    // print + the sentinel arrives.
    assert!(
        stdout.iter().any(|l| l.contains("FLUXV1_TOOLCHAIN_OK")),
        "missing FLUXV1_TOOLCHAIN_OK sentinel — toolchain check didn't reach the end. \
         stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("git version")),
        "git wasn't found inside the fluxv1 VM. stdout:\n{stdout:#?}"
    );
    assert!(
        stdout.iter().any(|l| l.contains("flux version")),
        "flux CLI wasn't found inside the fluxv1 VM. stdout:\n{stdout:#?}"
    );
    // `kustomize version` prints just `vX.Y.Z` (no "kustomize" prefix).
    // We pinned 5.x in the Dockerfile, so the bare presence of a `v5.` line
    // (matching the pinned major) is enough proof that the binary ran.
    assert!(
        stdout.iter().any(|l| {
            let t = l.trim();
            t.starts_with("v5.")
                && t[3..]
                    .split('.')
                    .next()
                    .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        }),
        "kustomize v5.x version line wasn't found inside the fluxv1 VM. stdout:\n{stdout:#?}"
    );

    Ok(())
}

/// Drive the actual deploy script end-to-end: clone (auto-init) a `file://`
/// bare repo inside the VM, write a manifest at the destination path
/// layout, commit, push. Asserts the `FLUX_PUSHED` sentinel appears.
///
/// Bare repo lives in the VM's tmpfs (`/tmp/forest-flux-remote.git`) — we
/// can't inspect it from the host, but `FLUX_PUSHED` is only printed when
/// `git push` succeeds, so its presence is the proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fluxv1_real_deploy_through_orchestrator() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/fluxv1/1").await?;

    let release_token = format!("tkn-flux-deploy-{}", short_token());

    // Ship a manifest. The dispatcher's filter strips
    // `<env>/<dest>/<org>/<type>@<v>/` so the file lands at
    // /work/service.yaml inside the VM, which the deploy script then
    // copies into the destination path layout.
    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "fluxv1-deploy".into(),
            release_files: vec![(
                "prod/fluxv1-deploy-dest/forest/fluxv1@1/service.yaml".to_string(),
                "apiVersion: v1\nkind: Service\nmetadata:\n  name: hello\n".to_string(),
            )],
            ..Default::default()
        },
    );

    let mut metadata = HashMap::new();
    metadata.insert(
        "git_url".to_string(),
        "file:///tmp/forest-flux-remote.git".to_string(),
    );
    metadata.insert("git_branch".to_string(), "main".to_string());
    metadata.insert("cluster_name".to_string(), "test-cluster".to_string());
    metadata.insert("namespace".to_string(), "default".to_string());

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-flux-deploy-1".into(),
        release_intent_id: "int-flux-deploy-1".into(),
        artifact_id: "art-flux-deploy-1".into(),
        destination_id: "dest-flux-deploy-1".into(),
        destination: Some(DestinationInfo {
            name: "fluxv1-deploy-dest".into(),
            environment: "prod".into(),
            metadata,
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "fluxv1".into(),
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

    if completion.outcome != ReleaseOutcome::Success {
        let relevant: Vec<_> = logs
            .iter()
            .filter(|l| l.channel != "console")
            .take(60)
            .collect();
        panic!(
            "expected SUCCESS, got {:?}: {}\nlogs (non-console, first 60):\n{:#?}",
            completion.outcome, completion.error_message, relevant
        );
    }

    let stdout: Vec<&str> = logs
        .iter()
        .filter(|l| l.channel == "stdout")
        .map(|l| l.line.as_str())
        .collect();

    assert!(
        stdout.iter().any(|l| l.contains("FLUX_PUSHED")),
        "missing FLUX_PUSHED sentinel — deploy script didn't reach a successful push. \
         stdout:\n{stdout:#?}"
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
