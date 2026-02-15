use anyhow::Context;
use base64::{Engine, prelude::BASE64_STANDARD};
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
    command: Commands,

    #[arg(long, env = "EXTERNAL_HOST")]
    external_host: String,

    #[arg(long, env = "FOREST_TERRAFORM_V1_EXTERNAL_HOST")]
    terraform_external_host: String,

    #[arg(long, env = "PASSWORD_SECRET_KEY")]
    password_secret_key: String,

    #[arg(long, env = "ACCESS_TOKEN_SECRET_KEY")]
    access_token_secret_key: String,

    #[arg(long, env = "REFRESH_TOKEN_SECRET_KEY")]
    refresh_token_secret_key: String,
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

    if cli.password_secret_key.len() != 32 {
        anyhow::bail!(
            "password-secret-key must be exactly 32 characters long is ({})",
            cli.password_secret_key.len()
        )
    }

    let config = Config {
        external_host: cli.external_host.clone(),
        terraform_external_host: cli.terraform_external_host.clone(),

        password_secret_key: cli.password_secret_key,
        refresh_token_secret_key: BASE64_STANDARD
            .decode(cli.refresh_token_secret_key)
            .context("refresh token secret was not base64")?,
        access_token_secret_key: BASE64_STANDARD
            .decode(cli.access_token_secret_key)
            .context("access token secret was not base64")?,
    };
    let state = State::new(config).await?;

    cli.command.execute(&state).await?;

    Ok(())
}
