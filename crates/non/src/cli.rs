use admin::AdminCommand;
use clap::{Parser, Subcommand};
use components::ComponentsCommand;
use global::GlobalCommand;
use init::InitCommand;
use publish::PublishCommand;
use run::RunCommand;
use template::TemplateCommand;

use crate::state::State;

mod admin;
mod components;
mod global;
mod init;
mod publish;
mod run;
mod template;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Init(InitCommand),
    Components(ComponentsCommand),
    Admin(AdminCommand),
    Run(RunCommand),
    Template(TemplateCommand),
    Publish(PublishCommand),
    Global(GlobalCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    tracing::debug!("starting cli");

    let state = State::new().await?;

    match cli
        .command
        .expect("commands are required should've been caught by clap")
    {
        Commands::Init(init_command) => init_command.execute(&state).await,
        Commands::Components(components_command) => components_command.execute(&state).await,
        Commands::Admin(cmd) => cmd.execute(&state).await,
        Commands::Run(cmd) => cmd.execute(&state).await,
        Commands::Template(cmd) => cmd.execute(&state).await,
        Commands::Publish(cmd) => cmd.execute(&state).await,
        Commands::Global(cmd) => cmd.execute(&state).await,
    }
}
