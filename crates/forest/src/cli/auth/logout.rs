use crate::state::State;

#[derive(clap::Parser)]
pub struct LogoutCommand {}

impl LogoutCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        // TODO: read stored refresh token from local config and call logout
        // state.grpc_client().logout(&refresh_token).await?;

        println!("Logged out");

        Ok(())
    }
}
