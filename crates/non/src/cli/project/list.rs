use crate::state::State;

mod namespace {
    use crate::{grpc::GrpcClientState, state::State};

    #[derive(clap::Parser)]
    pub struct NamespacesCommand {}

    impl NamespacesCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let namespaces = state.grpc_client().get_namespaces().await?;

            println!("namespaces:\n");
            for namespace in namespaces {
                println!("- {}", namespace.as_str())
            }

            Ok(())
        }
    }
}
mod project {
    use anyhow::Context;

    use crate::{grpc::GrpcClientState, state::State};

    #[derive(clap::Parser)]
    pub struct ProjectsCommand {
        #[arg(long)]
        destination: Option<String>,

        #[arg(long)]
        namespace: Option<String>,
    }

    impl ProjectsCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let projects = state
                .grpc_client()
                .get_projects(match (&self.destination, &self.namespace) {
                    (None, None) => anyhow::bail!("either a destination or namespace is required"),
                    (None, Some(ns)) => crate::grpc::GetProjectsQuery::Namespace(ns.clone().into()),
                    (Some(_dest), None) => todo!(),
                    (Some(_), Some(_)) => anyhow::bail!("a destination or namespace is required"),
                })
                .await
                .context("get projects")?;

            println!("projects:\n");
            for project in projects {
                println!("- {}", project.as_str());
            }

            Ok(())
        }
    }
}

mod destination {
    use crate::{grpc::GrpcClientState, state::State};

    #[derive(clap::Parser)]
    pub struct DestinationsCommand {}

    impl DestinationsCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let destinations = state.grpc_client().get_destinations().await?;

            println!("destinations:\n");
            for destination in destinations {
                println!("- {}", destination.as_str())
            }

            Ok(())
        }
    }
}

#[derive(clap::Parser)]
pub struct ListCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    #[clap(alias = "ns")]
    Namespaces(namespace::NamespacesCommand),
    Projects(project::ProjectsCommand),
    Destinations(destination::DestinationsCommand),
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Namespaces(cmd) => cmd.execute(state).await,
            Commands::Projects(cmd) => cmd.execute(state).await,
            Commands::Destinations(cmd) => cmd.execute(state).await,
        }
    }
}
