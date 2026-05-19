use anyhow::Context;
use serde::Serialize;
use tabled::Tabled;

use crate::{
    cli::output::{self, OutputFormat},
    grpc::GrpcClientState,
    state::State,
    user_state::UserStateLoaderState,
};

#[derive(clap::Parser)]
pub struct SearchCommand {
    /// Search query (matches against organisation name)
    query: String,

    /// Maximum number of results
    #[arg(long, default_value = "50")]
    page_size: i32,

    /// Pagination token from a previous search
    #[arg(long, default_value = "")]
    page_token: String,
}

#[derive(Tabled, Serialize)]
struct OrgRow {
    #[tabled(rename = "ID")]
    organisation_id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Created")]
    created_at: String,
}

impl SearchCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        let _user_state = state
            .user_state()
            .get_state()
            .await?
            .context("you must be logged in")?;

        let resp = state
            .grpc_client()
            .search_organisations(&self.query, self.page_size, &self.page_token)
            .await
            .context("failed to search organisations")?;

        if resp.organisations.is_empty() {
            match format {
                OutputFormat::Json => print!("[]"),
                _ => eprintln!("No organisations found"),
            }
            return Ok(());
        }

        let rows: Vec<OrgRow> = resp
            .organisations
            .iter()
            .map(|org| {
                let created_at = org
                    .created_at
                    .as_ref()
                    .and_then(|ts| chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32))
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default();

                OrgRow {
                    organisation_id: org.organisation_id.clone(),
                    name: org.name.clone(),
                    created_at,
                }
            })
            .collect();

        print!("{}", output::render(format, &rows));

        if !matches!(format, OutputFormat::Json) {
            eprintln!("{} total", resp.total_count);
            if !resp.next_page_token.is_empty() {
                eprintln!("Next page: --page-token {}", resp.next_page_token);
            }
        }

        Ok(())
    }
}
