use clap::{Parser, Subcommand};

use crate::state::State;

mod serve;
use serve::*;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Serve(ServeCommand),
}

impl Commands {
    async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match self {
            Commands::Serve(serve_command) => serve_command.execute(state).await,
        }
    }
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    tracing::debug!("starting cli");

    let state = State::new().await?;

    cli.command
        .expect("commands are required should've been caught by clap")
        .execute(&state)
        .await?;

    Ok(())
}
