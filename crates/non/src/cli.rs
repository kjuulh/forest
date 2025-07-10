use admin::AdminCommand;
use clap::{Parser, Subcommand};
use components::ComponentsCommand;
use global::GlobalCommand;
use init::InitCommand;
use publish::PublishCommand;
use run::RunCommand;
use shell::ShellCommand;
use template::TemplateCommand;
use tmp::TmpCommand;

use crate::state::State;

mod admin;
mod components;
mod global;
mod init;
mod publish;
mod run;
mod shell;
mod template;
mod tmp;

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
    Shell(ShellCommand),
    Tmp(TmpCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    let state = State::new().await?;

    let _state = state.clone();

    // TODO: Replace with mad at some point
    tokio::spawn(async move {
        let state = _state;

        loop {
            if let Err(e) = state.drop_queue.process().await {
                tracing::warn!("failed to process items: {}", e)
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    let res = match cli
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
        Commands::Shell(cmd) => cmd.execute(&state).await,
        Commands::Tmp(cmd) => cmd.execute(&state).await,
    };
    if let Err(e) = state.drop_queue.drain().await {
        tracing::warn!("failed to process dropped items: {}", e);
    }

    res
}
