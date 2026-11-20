use std::collections::BTreeMap;

use anyhow::Context;
use forest_models::Destination;
use serde::Serialize;
use tabled::Tabled;

use crate::{
    cli::output::{self, OutputFormat},
    grpc::GrpcClientState,
    state::State,
};

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

        let format = &state.config.format;

        if !matches!(format, OutputFormat::Pretty) {
            let rows: Vec<DestinationRow> = destinations.iter().map(DestinationRow::from).collect();
            if rows.is_empty() && matches!(format, OutputFormat::Json) {
                println!("[]");
            } else {
                print!("{}", output::render(format, &rows));
            }
            return Ok(());
        }

        if destinations.is_empty() {
            println!("No destinations added yet");

            return Ok(());
        }

        eprintln!("destinations\n");

        let mut hidden_example: Option<(String, String)> = None;

        for destination in &destinations {
            println!("{} @ {}", destination.environment, destination.name);
            println!("type: {}", destination.destination_type.qualified());

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

        if let Some((destination, key)) = hidden_example {
            eprintln!(
                "\nsome values are hidden. reveal one with:\n  forest destination reveal --org {} --name {} --key {}",
                self.organisation, destination, key
            );
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Tabled)]
struct DestinationRow {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Environment")]
    environment: String,
    #[tabled(rename = "Type")]
    #[serde(rename = "type")]
    destination_type: String,
    #[tabled(rename = "Metadata", display = "display_metadata")]
    metadata: BTreeMap<String, String>,
    #[tabled(skip)]
    sensitive_keys: Vec<String>,
}

impl From<&Destination> for DestinationRow {
    fn from(destination: &Destination) -> Self {
        let metadata = metadata_rows(destination)
            .into_iter()
            .map(|row| match row {
                MetadataRow::Visible { key, value } => (key, value),
                MetadataRow::Hidden { key } => (key, REDACTED.to_string()),
            })
            .collect();

        let mut sensitive_keys = destination.sensitive_keys.clone();
        sensitive_keys.sort();

        Self {
            name: destination.name.clone(),
            environment: destination.environment.clone(),
            destination_type: destination.destination_type.qualified(),
            metadata,
            sensitive_keys,
        }
    }
}

fn display_metadata(metadata: &BTreeMap<String, String>) -> String {
    metadata
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Debug, PartialEq, Eq)]
enum MetadataRow {
    Visible { key: String, value: String },
    Hidden { key: String },
}

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
    fn json_is_json_with_the_type_and_no_secret_values() {
        let dest = destination(&[("region", "eu-west-1")], &["git_token"]);
        let rows = vec![DestinationRow::from(&dest)];

        let json = output::render(&OutputFormat::Json, &rows);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        assert_eq!(parsed[0]["name"], "flux-dev");
        assert_eq!(parsed[0]["environment"], "dev");
        assert_eq!(parsed[0]["type"], "forest/flux@1");
        assert_eq!(parsed[0]["metadata"]["region"], "eu-west-1");
        assert_eq!(parsed[0]["metadata"]["git_token"], REDACTED);
        assert_eq!(parsed[0]["sensitive_keys"][0], "git_token");
    }

    #[test]
    fn name_format_is_just_the_names() {
        let rows = vec![DestinationRow::from(&destination(&[], &[]))];
        assert_eq!(output::render(&OutputFormat::Name, &rows), "flux-dev\n");
    }

    #[test]
    fn text_format_carries_the_type_column() {
        let rows = vec![DestinationRow::from(&destination(&[("a", "1")], &[]))];
        assert_eq!(
            output::render(&OutputFormat::Text, &rows),
            "flux-dev\tdev\tforest/flux@1\ta=1\n"
        );
    }

    #[test]
    fn no_metadata_produces_no_rows() {
        assert!(metadata_rows(&destination(&[], &[])).is_empty());
    }
}
