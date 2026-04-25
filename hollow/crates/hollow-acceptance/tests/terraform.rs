//! Real `forest/terraform/1` destination running inside a Firecracker microVM.
//!
//! The binary inside the image is OpenTofu (CLI-compatible, BSL-free); the
//! controller's literal `terraform init/plan/apply` commands resolve to it
//! via a `terraform → tofu` symlink in the rootfs. Drives the full
//! orchestrator path: fake forest-server → controller → agent → VM → guest.
//! Providers are baked into the rootfs and served through a filesystem
//! mirror, so this test does not exercise per-VM networking.

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
  content  = "from terraform-v1 in hollow"
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terraform_plan_through_orchestrator() -> anyhow::Result<()> {
    let harness = skip_unless_harness!();

    let mut orchestrator = harness.start_orchestrator("forest/terraform/1").await?;

    let release_token = format!("tkn-tf-{}", short_token());

    orchestrator.fake_server.install_fixture(
        &release_token,
        ReleaseFixture {
            organisation: "test-org".into(),
            project: "terraform-smoke".into(),
            // Match production layout: artefacts are packed under
            // <env>/<dest-name-or-pattern>/<org>/<type>@<version>/<file>.
            // The controller's dispatcher filters to this destination's
            // subset and strips the prefix before sending to the guest.
            release_files: vec![(
                "test/terraform-smoke-dest/forest/terraform@1/main.tf".into(),
                MAIN_TF.into(),
            )],
            ..Default::default()
        },
    );

    let assignment = WorkAssignment {
        release_token: release_token.clone(),
        release_id: "rel-tf-1".into(),
        release_intent_id: "int-tf-1".into(),
        artifact_id: "art-tf-1".into(),
        destination_id: "dest-tf-1".into(),
        destination: Some(DestinationInfo {
            name: "terraform-smoke-dest".into(),
            environment: "test".into(),
            metadata: HashMap::new(),
            r#type: Some(DestinationCapability {
                organisation: "forest".into(),
                name: "terraform".into(),
                version: 1,
            }),
            organisation: "forest".into(),
        }),
        // Plan mode: guest captures stdout and returns it in CompleteRelease.plan_output.
        mode: ReleaseMode::Plan.into(),
        // The puppet doesn't run a real state backend; the test config only
        // creates `null_resource` + `local_file` so plan-without-state is
        // well-defined.
        artifact_store: None,
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
