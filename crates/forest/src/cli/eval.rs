//! `forest eval zsh|bash` — emits the shell script that prepends the
//! global shim directory to `$PATH`. See TASKS/018-global-tools.md §1a.7.

use clap::{Parser, Subcommand};

use crate::global::eval::{eval_bash, eval_zsh};
use crate::state::State;

#[derive(Parser)]
pub struct EvalCommand {
    #[command(subcommand)]
    shell: EvalShell,
}

#[derive(Subcommand)]
pub enum EvalShell {
    /// Emit zsh integration script (eval into ~/.zshrc).
    Zsh,
    /// Emit bash integration script (eval into ~/.bashrc).
    Bash,
}

impl EvalCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let script = match self.shell {
            EvalShell::Zsh => eval_zsh(),
            EvalShell::Bash => eval_bash(),
        };
        print!("{script}");
        Ok(())
    }
}
