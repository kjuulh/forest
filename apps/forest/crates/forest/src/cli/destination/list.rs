use anyhow::Context;
use forest_models::Destination;

use crate::{grpc::GrpcClientState, state::State};

/// Stand-in printed instead of a credential. Same width regardless of the
/// value's real length, so the output leaks nothing about it.
const REDACTED: &str = "••••••••";

#[derive(clap::Parser)]
pub struct ListCommand {
    #[arg(long, short = 'o', visible_alias = "org")]
    organisation: String,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let destinations = state
            .grpc_client()
            .get_destinations(&self.organisation)
            .await
            .context("get destinations")?;

        if destinations.is_empty() {
            println!("No destinations added yet");

            return Ok(());
        }

        eprintln!("destinations\n");

        let mut hidden_example: Option<(String, String)> = None;

        for destination in &destinations {
            println!("{} @ {}", destination.environment, destination.name);

            let rows = metadata_rows(destination);
            if rows.is_empty() {
                continue;
            }

            println!("metadata:");
            for row in &rows {
                match row {
                    MetadataRow::Visible { key, value } => println!("  {key}: {value}"),
                    MetadataRow::Hidden { key } => {
                        println!("  {key}: {REDACTED}");
                        hidden_example
                            .get_or_insert_with(|| (destination.name.clone(), key.clone()));
                    }
                }
            }
        }

        // Name the escape hatch, but only when something was actually hidden.
        if let Some((destination, key)) = hidden_example {
            eprintln!(
                "\nsome values are hidden. reveal one with:\n  forest destination reveal --org {} --name {} --key {}",
                self.organisation, destination, key
            );
        }

        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MetadataRow {
    Visible { key: String, value: String },
    Hidden { key: String },
}

/// One row per metadata key, sorted so the output is stable across runs.
///
/// A key is hidden when the server withheld its value — which it does for keys
/// the destination type declares `sensitive` and for keys the destination
/// itself declares sensitive. Anything else is shown: sensitivity is declared,
/// never guessed from the key's name, so free-form extras stay visible unless
/// somebody marked them.
fn metadata_rows(destination: &Destination) -> Vec<MetadataRow> {
    let mut rows: Vec<MetadataRow> = destination
        .metadata
        .iter()
        .map(|(key, value)| MetadataRow::Visible {
            key: key.clone(),
            value: value.clone(),
        })
        .chain(
            destination
                .sensitive_keys
                .iter()
                .map(|key| MetadataRow::Hidden { key: key.clone() }),
        )
        .collect();

    rows.sort_by(|a, b| row_key(a).cmp(row_key(b)));
    rows
}

fn row_key(row: &MetadataRow) -> &str {
    match row {
        MetadataRow::Visible { key, .. } | MetadataRow::Hidden { key } => key,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use forest_models::{DestinationType, MetadataFieldSchema};

    use super::*;

    fn destination(metadata: &[(&str, &str)], sensitive_keys: &[&str]) -> Destination {
        Destination::new(
            "understory",
            "flux-dev",
            "dev",
            metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
            DestinationType {
                organisation: "forest".into(),
                name: "flux".into(),
                version: 1,
                description: String::new(),
                fields: Vec::<MetadataFieldSchema>::new(),
            },
        )
        .with_sensitive_keys(sensitive_keys.iter().map(|k| k.to_string()).collect())
    }

    fn rendered(destination: &Destination) -> Vec<String> {
        metadata_rows(destination)
            .iter()
            .map(|row| match row {
                MetadataRow::Visible { key, value } => format!("  {key}: {value}"),
                MetadataRow::Hidden { key } => format!("  {key}: {REDACTED}"),
            })
            .collect()
    }

    #[test]
    fn withheld_keys_render_as_a_placeholder_not_a_value() {
        let dest = destination(&[("cluster_name", "prod-eu")], &["git_token"]);

        assert_eq!(
            rendered(&dest),
            vec![
                "  cluster_name: prod-eu".to_string(),
                format!("  git_token: {REDACTED}"),
            ]
        );
    }

    #[test]
    fn undeclared_keys_stay_visible() {
        // The terraform case: keys the type never declares are forwarded as
        // TF_VAR_* and are not secret by default.
        let dest = destination(
            &[
                ("tf_workspace", "platform-dev"),
                ("infra_environment", "dev"),
            ],
            &[],
        );

        assert_eq!(
            rendered(&dest),
            vec![
                "  infra_environment: dev".to_string(),
                "  tf_workspace: platform-dev".to_string(),
            ]
        );
    }

    #[test]
    fn declared_free_form_keys_are_hidden() {
        // DATA-575: these live outside the terraform type's field schema.
        let dest = destination(
            &[
                ("tf_workspace", "platform-dev"),
                ("aws_account_id", "12345"),
            ],
            &[
                "aws_access_key_id",
                "aws_secret_access_key",
                "cloudflare_token",
            ],
        );

        assert_eq!(
            rendered(&dest),
            vec![
                format!("  aws_access_key_id: {REDACTED}"),
                "  aws_account_id: 12345".to_string(),
                format!("  aws_secret_access_key: {REDACTED}"),
                format!("  cloudflare_token: {REDACTED}"),
                "  tf_workspace: platform-dev".to_string(),
            ]
        );
    }

    #[test]
    fn output_is_ordered_regardless_of_map_iteration() {
        let dest = destination(
            &[("zulu", "1"), ("alpha", "2"), ("mike", "3")],
            &["bravo", "yankee"],
        );

        let rows = metadata_rows(&dest);
        let keys: Vec<&str> = rows.iter().map(row_key).collect();

        assert_eq!(keys, vec!["alpha", "bravo", "mike", "yankee", "zulu"]);
    }

    #[test]
    fn no_metadata_produces_no_rows() {
        assert!(metadata_rows(&destination(&[], &[])).is_empty());
    }
}
