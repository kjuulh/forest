use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State, user_config::UserConfigServiceState};

#[derive(clap::Parser, Debug)]
pub struct GlobalAddCommand {
    #[arg()]
    component: String,
}

struct AddComponent {
    namespace: String,
    name: String,
    version: Option<semver::Version>,
}

impl TryFrom<&GlobalAddCommand> for AddComponent {
    type Error = anyhow::Error;

    fn try_from(value: &GlobalAddCommand) -> Result<Self, Self::Error> {
        let (namespace, rest) = match value.component.split_once("/") {
            Some((namespace, rest)) => (namespace, rest),
            None => {
                // is non
                (
                    "non",
                    // Rest
                    value.component.as_str(),
                )
            }
        };

        let (name, version) = match rest.split_once("@") {
            Some((name, version)) => (
                name,
                Some(
                    version
                        .parse::<semver::Version>()
                        .context("failed to parse version as semver (non/init@v1.2.3)")?,
                ),
            ),
            None => (rest, None),
        };

        Ok(Self {
            namespace: namespace.into(),
            name: name.into(),
            version,
        })
    }
}

impl GlobalAddCommand {
    #[tracing::instrument(skip(state), level = "debug")]
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        tracing::info!("adding global dependency");
        let add_command: AddComponent = self.try_into()?;

        let component = match add_command.version {
            Some(version) => {
                state
                    .grpc_client()
                    .get_component_version(
                        &add_command.name,
                        &add_command.namespace,
                        &version.to_string(),
                    )
                    .await?
            }
            None => {
                state
                    .grpc_client()
                    .get_component(&add_command.name, &add_command.namespace)
                    .await?
            }
        };

        tracing::debug!("found version");

        let component = component.ok_or(anyhow::anyhow!("failed to find component"))?;

        state
            .user_config_service()
            .add_dependency(
                &add_command.name,
                &add_command.namespace,
                &component.version,
            )
            .await?;

        tracing::info!("added global dependency");

        Ok(())
    }
}
