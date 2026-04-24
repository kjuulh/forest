//! Real OpenTofu running inside a Firecracker microVM.
//!
//! Drives a `tofu init && tofu plan` through the full orchestrator path
//! (fake forest-server → controller → agent → VM → guest → `tofu`).
//! Providers are baked into the `opentofu-v1` rootfs image and served through
//! a filesystem_mirror, so this test does NOT require per-VM networking —
//! that's a separate milestone.

use std::collections::HashMap;
use std::time::Duration;

use forest_grpc_interface::{
    DestinationCapability, DestinationInfo, ReleaseMode, ReleaseOutcome, WorkAssignment,
};
use hollow_acceptance::skip_unless_harness;
use hollow_test_harness::fake_server::ReleaseFixture;

const MAIN_TF: &str = r#"
terraform {
  required_providers {
    null  = { source = "hashicorp/null",  version = "~> 3.2" }
    local = { source = "hashicorp/local", version = "~> 2.8" }
  }
}

resource "null_resource" "hello" {
  triggers = {
    name = "hollow-acceptance"
  }
}

resource "local_file" "hello" {
  filename = "/tmp/hollow-demo.txt"
  content  = "from opentofu in hollow"
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opentofu_plan_through_orchestrator() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/opentofu/1").await?;

    let release_token = format!("tkn-otf-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "opentofu-smoke".into(),
            release_files: vec![("main.tf".into(), MAIN_TF.into())],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-otf-1".into(),
        release_intent_id: "int-otf-1".into(),
        artifact_id: "art-otf-1".into(),
        destination_id: "dest-otf-1".into(),
        destination: Some(DestinationInfo {
            name: "opentofu-smoke-dest".into(),
            environment: "test".into(),
            metadata: HashMap::new(),
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "opentofu".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        // Plan mode: guest captures stdout and returns it in CompleteRelease.plan_output.
        mode: ReleaseMode::Plan.into(),
    };

    orchestrator.fake_server.dispatch(assignment)?;

    // `tofu init` + `tofu plan` inside a 1 GiB rootfs microVM takes a while
    // on first boot; give it plenty of headroom.
    let (completion, logs) = orchestrator
        .fake_server
        .wait_for_completion(&release_token, Duration::from_secs(120))
        .await?;

    // If the run failed, dump the relevant log channels so the failure
    // message is actually useful.
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

    let plan = completion
        .plan_output
        .as_deref()
        .expect("SUCCESS plan should have captured plan_output");

    for marker in [
        "null_resource.hello",
        "local_file.hello",
        "Plan:",
    ] {
        assert!(
            plan.contains(marker),
            "plan_output missing `{marker}`. plan_output:\n{plan}"
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
