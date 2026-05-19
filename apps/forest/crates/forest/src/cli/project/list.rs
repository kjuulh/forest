use crate::state::State;

mod organisation {
    use crate::{grpc::GrpcClientState, state::State};

    #[derive(clap::Parser)]
    pub struct OrganisationsCommand {}

    impl OrganisationsCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let organisations = state.grpc_client().get_organisations().await?;

            eprintln!("organisations:");
            eprintln!();
            for organisation in organisations {
                println!("{}", organisation.as_str())
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

        #[arg(long, short = 'o')]
        organisation: Option<String>,
    }

    impl ProjectsCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let projects = state
                .grpc_client()
                .get_projects(match (&self.destination, &self.organisation) {
                    (None, None) => anyhow::bail!("either a destination or organisation is required"),
                    (None, Some(org)) => crate::grpc::GetProjectsQuery::Organisation(org.clone().into()),
                    (Some(_dest), None) => todo!(),
                    (Some(_), Some(_)) => anyhow::bail!("only one of destination or organisation is required"),
                })
                .await
                .context("get projects")?;

            eprintln!("projects:");
            eprintln!();
            for project in projects {
                println!("{}", project.as_str());
            }

            Ok(())
        }
    }
}

mod destination {
    use crate::{grpc::GrpcClientState, state::State};

    #[derive(clap::Parser)]
    pub struct DestinationsCommand {
        #[arg(long, short = 'o')]
        organisation: String,
    }

    impl DestinationsCommand {
        pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
            let destinations = state
                .grpc_client()
                .get_destinations(&self.organisation)
                .await?;

            eprintln!("destinations:");
            eprintln!();
            for destination in destinations {
                println!("{}", destination)
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
    #[clap(alias = "orgs")]
    Organisations(organisation::OrganisationsCommand),
    Projects(project::ProjectsCommand),
    Destinations(destination::DestinationsCommand),
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Organisations(cmd) => cmd.execute(state).await,
            Commands::Projects(cmd) => cmd.execute(state).await,
            Commands::Destinations(cmd) => cmd.execute(state).await,
        }
    }
}
