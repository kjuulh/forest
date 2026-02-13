use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State, user_state::UserStateLoaderState};

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

impl SearchCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
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
            println!("No organisations found");
            return Ok(());
        }

        for org in &resp.organisations {
            println!("{}\t{}", org.organisation_id, org.name);
        }

        println!("\n{} total", resp.total_count);

        if !resp.next_page_token.is_empty() {
            println!("Next page: --page-token {}", resp.next_page_token);
        }

        Ok(())
    }
}
