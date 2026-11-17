use anyhow::Context;
use forest_grpc_interface::{DestinationDrift, DriftStatus};

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
/// Exit status: 0 every destination checked and in sync, 2 drift with no failed
/// checks, 1 an incomplete/unsupported/failed check (including an empty result).
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

        let code = report_drift(&resp.destinations, self.quiet);
        if code != 0 {
            std::process::exit(code);
        }
        Ok(())
    }
}

fn report_drift(destinations: &[DestinationDrift], quiet: bool) -> i32 {
    let mut worst = if destinations.is_empty() {
        eprintln!("drift check returned no destinations");
        1
    } else {
        0
    };
    for d in destinations {
        let (label, code) = match DriftStatus::try_from(d.status) {
            Ok(DriftStatus::InSync) => ("in sync", 0),
            Ok(DriftStatus::Drifted) => ("DRIFTED", 2),
            Ok(DriftStatus::Unsupported) => ("no drift check for this destination type", 1),
            _ => ("check failed", 1),
        };
        println!("{}: {label}", d.destination_name);
        if !quiet && !d.output.is_empty() && code == 2 {
            println!("{}\n", d.output);
        }
        if !d.error.is_empty() {
            println!("  {}", d.error);
        }
        // A partial result cannot certify that the requested scope was checked.
        worst = match (worst, code) {
            (1, _) | (_, 1) => 1,
            (2, _) | (_, 2) => 2,
            _ => 0,
        };
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(statuses: &[i32]) -> i32 {
        let destinations: Vec<_> = statuses
            .iter()
            .map(|&status| DestinationDrift {
                destination_name: "isolated".into(),
                status,
                ..Default::default()
            })
            .collect();
        report_drift(&destinations, true)
    }

    #[test]
    fn only_a_completed_in_sync_check_succeeds() {
        assert_eq!(check(&[DriftStatus::InSync as i32]), 0);
        assert_eq!(check(&[DriftStatus::Drifted as i32]), 2);
        assert_eq!(check(&[]), 1);
        for status in [
            DriftStatus::Unsupported as i32,
            DriftStatus::Error as i32,
            DriftStatus::Unspecified as i32,
            999,
        ] {
            assert_eq!(check(&[DriftStatus::InSync as i32, status]), 1);
        }
    }

    #[test]
    fn an_incomplete_scope_is_a_failure_even_when_another_destination_drifted() {
        for status in [DriftStatus::Unsupported as i32, DriftStatus::Error as i32] {
            assert_eq!(check(&[DriftStatus::Drifted as i32, status]), 1);
            assert_eq!(check(&[status, DriftStatus::Drifted as i32]), 1);
        }
    }
}
