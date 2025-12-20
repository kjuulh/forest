use clap::{Parser, Subcommand};

use crate::{Config, state::State};

mod serve;
use serve::*;

mod admin;
use admin::*;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long)]
    external_host: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    Serve(ServeCommand),
    Admin(AdminCommand),
}

impl Commands {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match self {
            Commands::Serve(cmd) => cmd.execute(state).await,
            Commands::Admin(cmd) => cmd.execute(state).await,
        }
    }
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    tracing::debug!("starting cli");

    let config = Config {
        external_host: cli.external_host.clone(),
    };
    let state = State::new(config).await?;

    cli.command
        .expect("commands are required should've been caught by clap")
        .execute(&state)
        .await?;

    Ok(())
}
