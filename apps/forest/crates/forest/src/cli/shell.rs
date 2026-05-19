use clap::{Parser, Subcommand};

use crate::state::State;

const ZSH_SCRIPT: &str = include_str!("scripts/forest.zsh");

#[derive(Parser)]
pub struct ShellCommand {
    #[command(subcommand)]
    subcommands: ShellCommands,
}

#[derive(Subcommand)]
pub enum ShellCommands {
    Zsh,
    Bash,
}

impl ShellCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match self.subcommands {
            ShellCommands::Zsh => println!("{}", ZSH_SCRIPT),
            ShellCommands::Bash => todo!(),
        }

        Ok(())
    }
}
