use anyhow::Context;
use forest_grpc_interface::DriftStatus;

use crate::{grpc::GrpcClientState, state::State};

/// Check whether what this project last deployed is still what is running.
///
/// Terraform state says what forest last applied; it does not say whether
/// someone has since changed the resource in a console. This asks, without
/// changing anything: for a `forest/terraform@1` destination the server runs
/// `terraform plan -refresh-only -detailed-exitcode -lock=false` against the
/// artifact of that destination's last successful deploy (forest#286).
///
///   forest release drift --environment dev
///   forest release drift -e prod -d prod/eu-west-1/core
///
/// Exit status: 0 in sync, 2 drift somewhere, 1 the check could not run — so
/// a scheduled job can alert on 2 without parsing anything.
#[derive(clap::Parser)]
pub struct DriftCommand {
    /// Environment to check.
    #[arg(long, short = 'e', alias = "env")]
    environment: String,

    /// One destination by name. Omit for every destination in the environment
    /// this project has deployed to.
    #[arg(long, short = 'd')]
    destination: Option<String>,

    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Print only the per-destination status lines, not the plan text.
    #[arg(long, short = 'q')]
    quiet: bool,
}

impl DriftCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (detected_org, detected_project) = super::detect::project(state).await;
        let organisation = self.organisation.clone().or(detected_org).context(
            "organisation not found: set project.organisation in forest.cue or pass --organisation",
        )?;
        let project =
            self.project.clone().or(detected_project).context(
                "project name not found: set project.name in forest.cue or pass --project",
            )?;

        eprintln!(
            "checking drift for {organisation}/{project} in {} …",
            self.environment
        );
        let resp = state
            .grpc_client()
            .check_drift(
                &organisation,
                &project,
                &self.environment,
                self.destination.as_deref(),
            )
            .await?;

        let mut worst = 0;
        for d in &resp.destinations {
            let (label, code) = match DriftStatus::try_from(d.status) {
                Ok(DriftStatus::InSync) => ("in sync", 0),
                Ok(DriftStatus::Drifted) => ("DRIFTED", 2),
                Ok(DriftStatus::Unsupported) => ("no drift check for this destination type", 0),
                _ => ("check failed", 1),
            };
            println!("{}: {label}", d.destination_name);
            if !self.quiet && !d.output.is_empty() && code == 2 {
                println!("{}\n", d.output);
            }
            if !d.error.is_empty() {
                println!("  {}", d.error);
            }
            // Drift outranks a failure elsewhere: it is the finding.
            worst = match (worst, code) {
                (2, _) | (_, 2) => 2,
                (1, _) | (_, 1) => 1,
                _ => 0,
            };
        }

        if worst != 0 {
            std::process::exit(worst);
        }
        Ok(())
    }
}
