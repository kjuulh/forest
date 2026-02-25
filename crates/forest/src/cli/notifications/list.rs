use super::format::format_notification;
use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ListCommand {
    /// Filter by organisation
    #[arg(long)]
    organisation: Option<String>,

    /// Filter by project
    #[arg(long)]
    project: Option<String>,

    /// Maximum number of notifications to return
    #[arg(long, default_value_t = 20)]
    limit: i32,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let client = state.grpc_client();

        let resp = client
            .list_notifications(
                self.limit,
                "",
                self.organisation.as_deref(),
                self.project.as_deref(),
            )
            .await?;

        if resp.notifications.is_empty() {
            println!("No notifications found.");
            return Ok(());
        }

        for notif in &resp.notifications {
            println!("{}\n", format_notification(notif));
        }

        Ok(())
    }
}
