use crate::{state::State, user_config::UserConfigServiceState};

#[derive(clap::Parser, Debug)]
pub struct SetCommand {
    #[arg()]
    key: String,
    #[arg()]
    value: String,
}

impl SetCommand {
    #[tracing::instrument(skip(state), level = "debug")]
    pub async fn execute(self, state: &State) -> anyhow::Result<()> {
        tracing::debug!("writing user keys to file");

        state
            .user_config_service()
            .set(&self.key, &self.value)
            .await?;

        tracing::debug!("done writing user keys to file");

        Ok(())
    }
}
