//! `forest shell zsh|bash` — emits a single shell-integration block.
//!
//! Combines the global-tools PATH-prepend (formerly `forest eval`) with the
//! shell helper functions (e.g. `forest-tmp`). Source it from your rc file:
//!
//!     eval "$(forest shell zsh)"   # or `bash`

use clap::{Parser, Subcommand};

use crate::global::eval::{eval_bash, eval_zsh};
use crate::state::State;

const ZSH_HELPERS: &str = include_str!("scripts/forest.zsh");
const BASH_HELPERS: &str = include_str!("scripts/forest.bash");

#[derive(Parser)]
pub struct ShellCommand {
    #[command(subcommand)]
    subcommands: ShellCommands,
}

#[derive(Subcommand)]
pub enum ShellCommands {
    /// Emit zsh integration (eval into ~/.zshrc).
    Zsh,
    /// Emit bash integration (eval into ~/.bashrc).
    Bash,
}

impl ShellCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match self.subcommands {
            ShellCommands::Zsh => {
                print!("{}", eval_zsh());
                print!("{}", ZSH_HELPERS);
            }
            ShellCommands::Bash => {
                print!("{}", eval_bash());
                print!("{}", BASH_HELPERS);
            }
        }
        Ok(())
    }
}
