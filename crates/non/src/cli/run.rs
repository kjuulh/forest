use std::process::Stdio;

use anyhow::Context;
use clap::Subcommand;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    non_context::{NonContext, NonContextState},
    services::project::ProjectParserState,
    state::State,
};

#[derive(clap::Parser)]
pub struct RunCommand {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(external_subcommand)]
    External(Vec<String>),
}

impl RunCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let ctx = state.context();

        let project = state.project_parser().get_project().await?;

        match &self.cmd {
            Commands::External(items) => {
                tracing::info!("item: {}", items.join(" "));

                match items.split_first() {
                    Some((command, _rest)) => {
                        match project
                            .commands
                            .iter()
                            .find(|(c, _)| c.command_name() == command)
                        {
                            Some((command_name, command)) => {
                                tracing::info!("running command: {}", command_name);

                                match command {
                                    crate::models::Command::Script(_) => {
                                        todo!("script files are not supported yet")
                                    }
                                    crate::models::Command::Inline(items) => {
                                        let mut cmd = tokio::process::Command::new("bash");
                                        cmd.arg("-c")
                                            .arg(format!(
                                                "set -e; \n\n # script begins here \n\n{}",
                                                items.join("\n")
                                            ))
                                            .stdout(Stdio::piped())
                                            .stderr(Stdio::piped())
                                            .current_dir(&project.path)
                                            .env(
                                                NonContext::get_context_key(),
                                                ctx.context_string(),
                                            )
                                            .env(
                                                NonContext::get_tmp_key(),
                                                ctx.get_tmp().await?.to_string(),
                                            );

                                        if let Ok(exe) = std::env::current_exe() {
                                            cmd.env("non", exe);
                                        }

                                        if let Some(comp) = command_name.to_component() {
                                            cmd.env(NonContext::get_component_key(), comp);
                                        }

                                        let mut proc = cmd.spawn().context("spawn child")?;

                                        if let Some(stdout) = proc.stdout.take() {
                                            tokio::spawn({
                                                let command_name = command_name.clone();
                                                async move {
                                                    let mut reader = BufReader::new(stdout).lines();

                                                    while let Ok(Some(line)) =
                                                        reader.next_line().await
                                                    {
                                                        println!(
                                                            "{}: {line}",
                                                            command_name.command_name()
                                                        )
                                                    }
                                                }
                                            });
                                        }

                                        if let Some(stderr) = proc.stderr.take() {
                                            tokio::spawn({
                                                let command_name = command_name.clone();
                                                async move {
                                                    let mut reader = BufReader::new(stderr).lines();

                                                    while let Ok(Some(line)) =
                                                        reader.next_line().await
                                                    {
                                                        println!(
                                                            "{}: {line}",
                                                            command_name.command_name()
                                                        )
                                                    }
                                                }
                                            });
                                        }

                                        if !proc
                                            .wait()
                                            .await
                                            .context("execute subcommand")?
                                            .success()
                                        {
                                            anyhow::bail!("command failed");
                                        }
                                    }
                                }
                            }
                            None => {
                                anyhow::bail!("no subcommand found with that name: {}", command)
                            }
                        }
                    }
                    None => {
                        anyhow::bail!("a subcommand is required");
                    }
                }
            }
        }

        Ok(())
    }
}
