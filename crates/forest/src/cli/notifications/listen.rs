use futures::StreamExt;

use super::format::format_notification;
use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ListenCommand {
    /// Filter by organisation
    #[arg(long, short = 'o', visible_alias = "org")]
    organisation: Option<String>,

    /// Filter by project
    #[arg(long)]
    project: Option<String>,
}

impl ListenCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        let mut stream = client
            .listen_notifications(
                self.organisation.as_deref(),
                self.project.as_deref(),
            )
            .await?;

        eprintln!("Listening for notifications...\n");

        while let Some(event) = stream.next().await {
            let notif = event.map_err(|e| anyhow::anyhow!("{}", e.message()))?;
            println!("{}\n", format_notification(&notif));
        }

        Ok(())
    }
}
