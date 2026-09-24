use std::collections::HashMap;

use anyhow::Context;
use forest_models::DestinationType;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: String,

    #[arg(long)]
    name: String,

    #[arg(long)]
    environment: String,

    #[arg(long = "type")]
    r#type: String,

    #[arg(long = "metadata")]
    metadata: Vec<String>,

    /// Treat this metadata key as a credential: its value is withheld from
    /// `destination list` and must be fetched with `destination reveal`.
    /// Repeatable. Keys the destination type already declares sensitive (e.g.
    /// flux `git_token`) need no flag — use this for free-form keys such as
    /// terraform's `TF_VAR_*` credentials.
    #[arg(long = "sensitive", visible_alias = "sensitive-key")]
    sensitive: Vec<String>,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (organisation, rest) = self
            .r#type
            .split_once("/")
            .ok_or(anyhow::anyhow!("an organisation and name is required"))?;
        let (name, version) = rest
            .split_once("@")
            .ok_or(anyhow::anyhow!("a name and version is required"))?;

        let version: usize = version
            .parse()
            .context("version is required to be a unsigned integer")?;

        let metadata = self
            .metadata
            .iter()
            .map(|m| {
                m.split_once("=")
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .ok_or(anyhow::anyhow!("metadata requires a 'key=value'"))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let widened = state
            .grpc_client()
            .create_destination(
                &self.organisation,
                &self.name,
                &self.environment,
                metadata,
                self.sensitive.clone(),
                DestinationType {
                    organisation: organisation.into(),
                    name: name.into(),
                    version,
                    description: String::new(),
                    fields: vec![],
                },
            )
            .await
            .context("create destination")?;

        if let Some(warning) = widened_warning(&self.environment, &self.name, &widened) {
            eprintln!("{warning}");
        }

        Ok(())
    }
}

/// The projects a new destination was just added to without asking for it.
///
/// A project that declares no destinations for an environment is released to
/// every destination there of a kind it renders, so these projects' next
/// release into `environment` will also target the new one (forest#288). That
/// is sometimes exactly what is wanted and sometimes a surprise; either way it
/// should not be silent. On stderr, so it never mixes into piped output.
fn widened_warning(environment: &str, destination: &str, widened: &[String]) -> Option<String> {
    if widened.is_empty() {
        return None;
    }

    let mut out = format!(
        "warning: {} release{} into '{environment}' without declaring destinations for it, so {} next release there will also target '{destination}':\n",
        match widened.len() {
            1 => "1 project".to_string(),
            n => format!("{n} projects"),
        },
        if widened.len() == 1 { "s" } else { "" },
        if widened.len() == 1 { "its" } else { "their" },
    );
    for project in widened {
        out.push_str(&format!("  {project}\n"));
    }
    out.push_str(&format!(
        "to keep a project off it, declare its destinations for '{environment}' in forest.cue"
    ));

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::widened_warning;

    #[test]
    fn no_widened_projects_means_no_warning() {
        assert_eq!(widened_warning("dev", "dev/eu-west-1/core", &[]), None);
    }

    #[test]
    fn the_warning_names_every_widened_project_and_the_way_out() {
        let warning = widened_warning(
            "dev",
            "dev/eu-west-1/core",
            &["understory/a".into(), "understory/b".into()],
        )
        .expect("a warning");

        assert!(
            warning.contains("2 projects release into 'dev'"),
            "{warning}"
        );
        assert!(
            warning.contains("  understory/a\n  understory/b\n"),
            "{warning}"
        );
        assert!(warning.contains("'dev/eu-west-1/core'"), "{warning}");
        assert!(
            warning.contains("declare its destinations for 'dev'"),
            "{warning}"
        );
    }

    #[test]
    fn one_project_reads_in_the_singular() {
        let warning = widened_warning("dev", "x", &["understory/a".into()]).unwrap();
        assert!(
            warning.contains("1 project releases into 'dev'"),
            "{warning}"
        );
        assert!(warning.contains("its next release"), "{warning}");
    }
}
