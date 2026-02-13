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
pub struct ListCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: Option<String>,

    /// Maximum number of results
    #[arg(long, default_value = "50")]
    page_size: i32,

    /// Pagination token from a previous list
    #[arg(long, default_value = "")]
    page_token: String,
}

#[derive(Tabled, Serialize)]
struct MemberRow {
    #[tabled(rename = "User ID")]
    user_id: String,
    #[tabled(rename = "Username")]
    username: String,
    #[tabled(rename = "Role")]
    role: String,
    #[tabled(rename = "Joined")]
    joined_at: String,
}

impl ListCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        let _user_state = state
            .user_state()
            .get_state()
            .await?
            .context("you must be logged in")?;

        let org_id = match &self.org {
            Some(o) => super::resolve_org_id(state, o).await?,
            None => super::prompt_org_select(state, "").await?,
        };

        let resp = state
            .grpc_client()
            .list_organisation_members(&org_id, self.page_size, &self.page_token)
            .await
            .context("failed to list members")?;

        if resp.members.is_empty() {
            match format {
                OutputFormat::Json => print!("[]"),
                _ => println!("No members found"),
            }
            return Ok(());
        }

        let rows: Vec<MemberRow> = resp
            .members
            .iter()
            .map(|m| {
                let joined_at = m
                    .joined_at
                    .as_ref()
                    .and_then(|ts| chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32))
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default();

                MemberRow {
                    user_id: m.user_id.clone(),
                    username: m.username.clone(),
                    role: m.role.clone(),
                    joined_at,
                }
            })
            .collect();

        print!("{}", output::render(format, &rows));

        if !matches!(format, OutputFormat::Json) {
            println!("{} total", resp.total_count);
            if !resp.next_page_token.is_empty() {
                println!("Next page: --page-token {}", resp.next_page_token);
            }
        }

        Ok(())
    }
}
