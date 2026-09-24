//! `CheckDrift` (forest#286): compare what a project last deployed with what is
//! actually running, without changing either.
//!
//! Terraform state records what forest last applied, which is not the same
//! question as what is running now — anyone with console access can change a
//! resource, and nothing would say so until the next release's plan shows a
//! diff nobody asked for. This answers it on demand, per destination, against
//! the artifact of that destination's last successful deploy.
//!
//! Authorization happens in the handler (`release.rs::check_drift`) before this
//! runs; everything here assumes the caller may see the project.

use anyhow::Context;
use forest_grpc_interface::{CheckDriftRequest, CheckDriftResponse, DestinationDrift, DriftStatus};
use uuid::Uuid;

use crate::{
    State,
    destination_services::DestinationServicesState,
    destinations::{DriftReport, logger::DestinationLogger},
    services::{destination_registry::DestinationRegistryState, release_registry::ReleaseItem},
};

pub(crate) async fn check_drift(
    state: &State,
    req: CheckDriftRequest,
) -> anyhow::Result<CheckDriftResponse> {
    let project = req.project.context("project is required")?;
    anyhow::ensure!(!req.environment.is_empty(), "environment is required");

    let project_id = sqlx::query_scalar!(
        "SELECT id FROM projects WHERE organisation = $1 AND project = $2",
        project.organisation,
        project.project,
    )
    .fetch_optional(&state.db)
    .await
    .context("look up project")?
    .with_context(|| {
        format!(
            "project {}/{} not found",
            project.organisation, project.project
        )
    })?;

    // The last successful deploy per destination in this environment. Its
    // artifact is what the destination is supposed to be running.
    let deployed = sqlx::query!(
        r#"
            SELECT DISTINCT ON (rs.destination_id)
                rs.release_id, rs.release_intent_id, rs.destination_id, rs.artifact_id,
                d.name AS destination_name
            FROM release_states rs
            JOIN destinations d ON d.id = rs.destination_id
            WHERE rs.project_id = $1
              AND rs.mode = 'deploy'
              AND rs.status = 'SUCCEEDED'
              AND d.environment = $2
              AND ($3 = '' OR d.name = $3)
            ORDER BY rs.destination_id, rs.completed_at DESC NULLS LAST, rs.queued_at DESC
        "#,
        project_id,
        req.environment,
        req.destination,
    )
    .fetch_all(&state.db)
    .await
    .context("find the last deploy per destination")?;

    if deployed.is_empty() {
        let scope = if req.destination.is_empty() {
            format!("environment '{}'", req.environment)
        } else {
            format!("destination '{}' in '{}'", req.destination, req.environment)
        };
        anyhow::bail!(
            "{}/{} has no successful deploy to {scope} — nothing to compare against",
            project.organisation,
            project.project
        );
    }

    let registry = state.destination_registry();
    let services = state.destination_services();

    let mut destinations = Vec::new();
    for row in deployed {
        let result = async {
            let dest = registry
                .get(&row.destination_id)
                .await?
                .context("destination no longer exists")?;
            let Some(svc) = services.get_destination(
                &dest.destination_type.organisation,
                &dest.destination_type.name,
                dest.destination_type.version,
            ) else {
                return Ok(None);
            };

            let deployed = ReleaseItem {
                id: row.release_id,
                release_intent_id: row.release_intent_id,
                artifact: row.artifact_id,
                project_id,
                destination_id: row.destination_id,
                status: "SUCCEEDED".into(),
                project: project.project.clone(),
            };
            svc.drift(&DestinationLogger::detached(), &deployed, &dest)
                .await
        }
        .await;

        destinations.push(to_proto(row.destination_name, row.artifact_id, result));
    }

    Ok(CheckDriftResponse { destinations })
}

fn to_proto(
    destination_name: String,
    artifact_id: Uuid,
    result: anyhow::Result<Option<Vec<DriftReport>>>,
) -> DestinationDrift {
    let base = DestinationDrift {
        destination_name,
        artifact_id: artifact_id.to_string(),
        ..Default::default()
    };
    match result {
        Ok(None) => DestinationDrift {
            status: DriftStatus::Unsupported as i32,
            ..base
        },
        Ok(Some(reports)) => {
            let drifted = reports.iter().any(|r| r.drifted);
            DestinationDrift {
                status: if drifted {
                    DriftStatus::Drifted
                } else {
                    DriftStatus::InSync
                } as i32,
                output: join_reports(&reports),
                ..base
            }
        }
        Err(e) => DestinationDrift {
            status: DriftStatus::Error as i32,
            error: format!("{e:#}"),
            ..base
        },
    }
}

fn join_reports(reports: &[DriftReport]) -> String {
    if let [only] = reports {
        return only.output.clone();
    }
    reports
        .iter()
        .map(|r| format!("── {} ──\n{}", r.work_dir, r.output))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(dir: &str, drifted: bool) -> DriftReport {
        DriftReport {
            work_dir: dir.into(),
            drifted,
            output: format!("output of {dir}"),
        }
    }

    #[test]
    fn one_drifted_root_makes_the_destination_drifted() {
        let d = to_proto(
            "dev/x".into(),
            Uuid::nil(),
            Ok(Some(vec![report("a", false), report("b", true)])),
        );
        assert_eq!(d.status, DriftStatus::Drifted as i32);
        assert!(
            d.output.contains("── a ──") && d.output.contains("── b ──"),
            "{}",
            d.output
        );
    }

    #[test]
    fn every_root_in_sync_is_in_sync() {
        let d = to_proto(
            "dev/x".into(),
            Uuid::nil(),
            Ok(Some(vec![report("a", false)])),
        );
        assert_eq!(d.status, DriftStatus::InSync as i32);
        assert_eq!(d.output, "output of a", "a single root is not labelled");
    }

    #[test]
    fn a_type_without_a_drift_check_says_so_rather_than_in_sync() {
        let d = to_proto("dev/x".into(), Uuid::nil(), Ok(None));
        assert_eq!(d.status, DriftStatus::Unsupported as i32);
    }

    #[test]
    fn a_failed_check_is_an_error_not_in_sync() {
        let d = to_proto(
            "dev/x".into(),
            Uuid::nil(),
            Err(anyhow::anyhow!("no credentials")),
        );
        assert_eq!(d.status, DriftStatus::Error as i32);
        assert!(d.error.contains("no credentials"));
    }
}
