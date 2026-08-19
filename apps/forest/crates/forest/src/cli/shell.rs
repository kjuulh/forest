//! `forest shell zsh|bash` — emits a single shell-integration block.
//!
//! Combines the global-tools PATH-prepend (formerly `forest eval`) with the
//! shell helper functions (e.g. `forest-tmp`). Source it from your rc file:
//!
//!     eval "$(forest shell zsh)"   # or `bash`
//!
//! `forest shell install` goes further: it writes the PATH-prepend into the
//! shell env file that non-interactive shells actually read (`~/.zshenv` for
//! zsh, `~/.bashrc` for bash). This is what makes forest tools discoverable to
//! shells **spawned** by other tools (e.g. Claude Code running `zsh -c` /
//! `bash -c`), which skip the interactive-only `~/.zshrc`. Idempotent and
//! reversible (`forest shell uninstall`). See `global::shellenv` (pure block
//! generator) + `global::install` (rc-file writer).

use clap::{Args, Parser, Subcommand};

use crate::global::eval::{
    eval_bash, eval_fish, eval_zsh, fish_shell_integration_block, shell_integration_block,
};
use crate::global::install;
use crate::state::State;

const ZSH_HELPERS: &str = include_str!("scripts/forest.zsh");
const BASH_HELPERS: &str = include_str!("scripts/forest.bash");
const FISH_HELPERS: &str = include_str!("scripts/forest.fish");

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
    /// Emit fish integration (source from ~/.config/fish/config.fish).
    Fish,
    /// Add the shim dir to PATH in your shell env files (~/.zshenv, ~/.bashrc)
    /// so tools that spawn non-interactive shells (which skip ~/.zshrc) can
    /// find forest-installed tools. Idempotent; reversible via `uninstall`.
    Install(InstallArgs),
    /// Undo `forest shell install` — removes the managed block it wrote.
    Uninstall,
}

#[derive(Args)]
pub struct InstallArgs {
    /// Print what would be written (target files + the managed block) without
    /// touching disk.
    #[arg(long)]
    dry_run: bool,
}

impl ShellCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match &self.subcommands {
            // Order matters: PATH first (so the shim dir is reachable), then the
            // helper functions, then the integration block — which *calls* one
            // of those helpers (`forest-defer-aggregate`) on a cold cache, so it
            // has to come after they are defined.
            ShellCommands::Zsh => {
                print!("{}", eval_zsh());
                print!("{}", ZSH_HELPERS);
                print!("{}", shell_integration_block("zsh"));
            }
            ShellCommands::Bash => {
                print!("{}", eval_bash());
                print!("{}", BASH_HELPERS);
                print!("{}", shell_integration_block("bash"));
            }
            ShellCommands::Fish => {
                print!("{}", eval_fish());
                print!("{}", FISH_HELPERS);
                print!("{}", fish_shell_integration_block());
            }
            ShellCommands::Install(args) => run_install(args).await?,
            ShellCommands::Uninstall => run_uninstall().await?,
        }
        Ok(())
    }
}

async fn run_install(args: &InstallArgs) -> anyhow::Result<()> {
    let (home, shells) = install::resolve_targets()?;

    if args.dry_run {
        print!("{}", install::render_dry_run(&home, &shells));
        eprintln!("(dry-run — nothing written)");
        return Ok(());
    }

    for shell in &shells {
        let rc = shell.rc_file(&home);
        match install::apply(*shell, &rc).await? {
            install::Applied::Added(p) => eprintln!("  + {} ({})", p.display(), shell.name()),
            install::Applied::Updated(p) => eprintln!("  ~ {} ({})", p.display(), shell.name()),
            install::Applied::Unchanged(p) => {
                eprintln!("  = {} ({}, already current)", p.display(), shell.name())
            }
        }
    }
    eprintln!(
        "forest tools are now on PATH for newly spawned shells. Open a new shell \
         (or `source` the file) to pick it up in this session."
    );
    Ok(())
}

async fn run_uninstall() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home dir is unset"))?;
    let mut removed = 0usize;
    for shell in install::all_shells() {
        let rc = shell.rc_file(&home);
        match install::uninstall(&rc).await? {
            install::Removed::Removed(p) => {
                removed += 1;
                eprintln!("  − {} ({})", p.display(), shell.name());
            }
            install::Removed::Absent(p) => {
                eprintln!("  · {} ({}, no managed block)", p.display(), shell.name())
            }
        }
    }
    eprintln!("removed {removed} managed block(s).");
    Ok(())
}
