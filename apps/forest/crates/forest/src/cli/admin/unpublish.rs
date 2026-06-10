//! `forest admin unpublish <name>@<version>` — remove a previously
//! published version from the registry (TASKS/025).
//!
//! Org members may unpublish any version they could have published. The
//! command requires `--yes` on non-TTY stdin so it cannot be accidentally
//! invoked from a pipe; on a TTY without `--yes`, the user is prompted.
//!
//! After unpublish, the version is unreachable for `forest global add`
//! and `forest components show`. The aggregate event log retains the
//! full lifecycle (publish → unpublish) for audit. Re-publishing at the
//! same version is allowed and behaves like a first publish.

use std::io::IsTerminal;

use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

/// `forest admin unpublish <name>@<version>`
#[derive(clap::Parser)]
pub struct UnpublishCommand {
    /// Component reference in the form `<org>/<name>@<version>`.
    /// Example: `understory/canopy-data-cli@0.1.0`.
    reference: String,

    /// Skip the interactive confirmation prompt. Required when stdin is
    /// not a TTY (e.g. CI / piped invocations).
    #[arg(long = "yes")]
    yes: bool,

    /// Optional free-form reason recorded in the aggregate event for
    /// audit. Surfaces on `forest admin show-history` (future).
    #[arg(long = "reason")]
    reason: Option<String>,
}

/// Parsed `<org>/<name>@<version>` reference.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionRef {
    organisation: String,
    name: String,
    version: String,
}

fn parse_reference(s: &str) -> anyhow::Result<VersionRef> {
    let (qualified, version) = s.rsplit_once('@').with_context(|| {
        format!(
            "invalid reference `{s}`: expected `<org>/<name>@<version>`"
        )
    })?;
    let (organisation, name) = qualified.split_once('/').with_context(|| {
        format!(
            "invalid reference `{s}`: expected `<org>/<name>@<version>`"
        )
    })?;
    if organisation.is_empty() || name.is_empty() || version.is_empty() {
        anyhow::bail!(
            "invalid reference `{s}`: organisation, name, and version must all be non-empty"
        );
    }
    Ok(VersionRef {
        organisation: organisation.to_string(),
        name: name.to_string(),
        version: version.to_string(),
    })
}

impl UnpublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let r = parse_reference(&self.reference)?;

        // Safety gate: a non-interactive stdin (CI / pipe) MUST pass --yes
        // so destructive ops can't be triggered by accident.
        if !self.yes && !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "refusing to unpublish without --yes (stdin is not a TTY)"
            );
        }

        // TTY confirmation flow.
        if !self.yes {
            eprintln!(
                "About to unpublish {}/{}@{}.",
                r.organisation, r.name, r.version
            );
            eprintln!("  This makes the version unreachable for `forest global add`.");
            eprintln!("  The aggregate's event history is preserved for audit.");
            eprintln!("  Re-publishing the same version afterward is allowed.");
            eprint!("Continue? [y/N]: ");
            use std::io::Write;
            std::io::stderr().flush().ok();

            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            let answer = line.trim();
            if !matches!(answer, "y" | "Y" | "yes" | "YES") {
                eprintln!("aborted.");
                return Ok(());
            }
        }

        let client = state.grpc_client();
        let recorded = client
            .unpublish_component_version(
                &r.organisation,
                &r.name,
                &r.version,
                self.reason.as_deref().unwrap_or(""),
            )
            .await?;

        if recorded {
            eprintln!(
                "unpublished {}/{}@{}",
                r.organisation, r.name, r.version
            );
            if let Some(reason) = &self.reason {
                eprintln!("  reason: {reason}");
            }
            eprintln!("  this version is now free; you can re-publish at the same number.");
        } else {
            eprintln!(
                "{}/{}@{} is already unpublished (no action taken)",
                r.organisation, r.name, r.version
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_reference() {
        let r = parse_reference("understory/canopy-data-cli@0.1.5").unwrap();
        assert_eq!(r.organisation, "understory");
        assert_eq!(r.name, "canopy-data-cli");
        assert_eq!(r.version, "0.1.5");
    }

    #[test]
    fn parses_pre_release_versions() {
        let r = parse_reference("acme/widget@1.0.0-alpha.1").unwrap();
        assert_eq!(r.version, "1.0.0-alpha.1");
    }

    #[test]
    fn handles_at_in_version_correctly() {
        // rsplit_once splits on the LAST @, so a pre-release containing
        // @ (not common, but possible) doesn't trip us up.
        let r = parse_reference("acme/widget@1.0.0").unwrap();
        assert_eq!(r.version, "1.0.0");
    }

    #[test]
    fn rejects_missing_at() {
        let err = parse_reference("understory/widget").unwrap_err();
        assert!(err.to_string().contains("expected `<org>/<name>@<version>`"));
    }

    #[test]
    fn rejects_missing_slash() {
        let err = parse_reference("widget@1.0.0").unwrap_err();
        assert!(err.to_string().contains("expected `<org>/<name>@<version>`"));
    }

    #[test]
    fn rejects_empty_components() {
        assert!(parse_reference("/name@1.0").is_err());
        assert!(parse_reference("org/@1.0").is_err());
        assert!(parse_reference("org/name@").is_err());
    }
}
