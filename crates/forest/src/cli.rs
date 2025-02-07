use std::net::SocketAddr;

use clap::{Parser, Subcommand};

use crate::state::SharedState;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Serve {
        #[arg(env = "FOREST_HOST", long, default_value = "127.0.0.1:3000")]
        host: SocketAddr,
    },
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();

    if let Some(Commands::Serve { .. }) = cli.command {
        tracing::info!("Starting forest server");

        let state = SharedState::new().await?;
    }

    Ok(())
}
