use crate::{cli::components::visibility::Visibility, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct VisibilityCommand {
    organisation: String,

    visibility: Visibility,
}

impl VisibilityCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let visibility = self.visibility.as_str();

        state
            .grpc_client()
            .set_organisation_visibility(&self.organisation, visibility)
            .await?;

        println!("{} is now {visibility}", self.organisation);
        Ok(())
    }
}
