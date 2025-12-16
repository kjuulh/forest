use std::process::Stdio;

use anyhow::Context;
use clap::{Arg, ArgAction, ArgMatches, Args, FromArgMatches};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    forest_context::{ForestContext, ForestContextState},
    models::Project,
    services::project::ProjectParserState,
    state::State,
};

pub struct RunCommand {
    args: Vec<String>,
}

impl FromArgMatches for RunCommand {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let args = matches
            .get_raw("args")
            .unwrap_or_default()
            .map(|i| i.to_string_lossy().to_string())
            .collect();

        Ok(Self { args })
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;

        Ok(())
    }
}

impl Args for RunCommand {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.disable_help_flag(true).arg(
            Arg::new("args")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .trailing_var_arg(true),
        )
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

impl RunCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let ctx = state.context();

        let project = state.project_parser().get_project().await?;

        let mut run_cmd = clap::Command::new("run").subcommand_required(true);
        for command_name in project.commands.keys() {
            let cmd = clap::Command::new(command_name.command_name().to_string());

            // TODO: add args

            run_cmd = run_cmd.subcommand(cmd);
        }

        let cmd = clap::Command::new("forest").subcommand(run_cmd);

        let mut args = Vec::new();
        args.push("forest".to_string());
        args.push("run".to_string());

        for arg in &self.args {
            args.push(arg.clone());
        }

        tracing::trace!("item: {}", self.args.join(" "));
        let matches = cmd.try_get_matches_from(args)?;

        let (_, matches) = matches
            .subcommand()
            .ok_or(anyhow::anyhow!("run command is required"))?;

        CliRun.execute(&ctx, &project, matches).await?;

        Ok(())
    }
}

struct CliRun;
impl CliRun {
    pub async fn execute(
        &self,
        ctx: &ForestContext,
        project: &Project,
        matches: &ArgMatches,
    ) -> anyhow::Result<()> {
        let (subcommand, _args) = matches
            .subcommand()
            .ok_or(anyhow::anyhow!("subcommand required"))?;

        tracing::info!(
            "register commands {}, [{}]",
            subcommand,
            project
                .commands
                .keys()
                .map(|c| c.command_name().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let (command_name, command) = project
            .commands
            .iter()
            .find(|(c, _)| c.command_name() == subcommand)
            .ok_or(anyhow::anyhow!("found no matching command"))?;

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
                    .env(ForestContext::get_context_key(), ctx.context_string())
                    .env(ForestContext::get_tmp_key(), ctx.get_tmp().await?.to_string());

                if let Ok(exe) = std::env::current_exe() {
                    cmd.env("forest", exe);
                }

                if let Some(comp) = command_name.to_component() {
                    cmd.env(ForestContext::get_component_key(), comp);
                }

                let mut proc = cmd.spawn().context("spawn child")?;

                if let Some(stdout) = proc.stdout.take() {
                    tokio::spawn({
                        let command_name = command_name.clone();
                        async move {
                            let mut reader = BufReader::new(stdout).lines();

                            while let Ok(Some(line)) = reader.next_line().await {
                                println!("{}: {line}", command_name.command_name())
                            }
                        }
                    });
                }

                if let Some(stderr) = proc.stderr.take() {
                    tokio::spawn({
                        let command_name = command_name.clone();
                        async move {
                            let mut reader = BufReader::new(stderr).lines();

                            while let Ok(Some(line)) = reader.next_line().await {
                                println!("{}: {line}", command_name.command_name())
                            }
                        }
                    });
                }

                if !proc.wait().await.context("execute subcommand")?.success() {
                    anyhow::bail!("command failed");
                }
            }
        }

        Ok(())
    }
}
