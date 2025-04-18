use std::path::Path;

use crate::{model::Context, script::ScriptExecutor};

// Run is a bit special in that because the arguments dynamically render, we need to do some special magic in
// clap to avoid having to do hacks to register clap subcommands midcommand. As such instead, we opt to simply
// create a new sub command that encapsulates all the run complexities
pub struct Run {}
impl Run {
    pub fn augment_command(ctx: &Context) -> clap::Command {
        let mut run_cmd = clap::Command::new("run")
            .subcommand_required(true)
            .about("runs any kind of script from either the project or plan");

        if let Some(scripts) = &ctx.project.scripts {
            for name in scripts.items.keys() {
                let cmd = clap::Command::new(name.to_string());
                run_cmd = run_cmd.subcommand(cmd);
            }
        }

        if let Some(plan) = &ctx.plan {
            if let Some(scripts) = &plan.scripts {
                let existing_cmds = run_cmd
                    .get_subcommands()
                    .map(|s| s.get_name().to_string())
                    .collect::<Vec<_>>();

                for name in scripts.items.keys() {
                    if existing_cmds.contains(name) {
                        continue;
                    }

                    let cmd = clap::Command::new(name.to_string());
                    run_cmd = run_cmd.subcommand(cmd);
                }
            }
        }

        run_cmd
    }

    pub fn augment_workspace_command(ctx: &Context, prefix: &str) -> Vec<clap::Command> {
        let mut commands = Vec::new();
        if let Some(scripts) = &ctx.project.scripts {
            for name in scripts.items.keys() {
                let cmd = clap::Command::new(format!("{prefix}::{name}"));
                commands.push(cmd);
            }
        }

        if let Some(plan) = &ctx.plan {
            if let Some(scripts) = &plan.scripts {
                let existing_cmds = commands
                    .iter()
                    .map(|s| format!("{prefix}::{}", s.get_name()))
                    .collect::<Vec<_>>();

                for name in scripts.items.keys() {
                    if existing_cmds.contains(name) {
                        continue;
                    }

                    let cmd = clap::Command::new(format!("{prefix}::{name}"));
                    commands.push(cmd)
                }
            }
        }

        commands
    }

    pub async fn execute(
        args: &clap::ArgMatches,
        project_path: &Path,
        ctx: &Context,
    ) -> anyhow::Result<()> {
        let Some((name, args)) = args.subcommand() else {
            anyhow::bail!("failed to find a matching run command")
        };

        if let Some(scripts_ctx) = &ctx.project.scripts {
            if let Some(script_ctx) = scripts_ctx.items.get(name) {
                ScriptExecutor::new(project_path.into(), ctx.clone())
                    .run(script_ctx, name)
                    .await?;

                return Ok(());
            }
        }

        if let Some(plan) = &ctx.plan {
            if let Some(scripts_ctx) = &plan.scripts {
                if let Some(script_ctx) = scripts_ctx.items.get(name) {
                    ScriptExecutor::new(project_path.into(), ctx.clone())
                        .run(script_ctx, name)
                        .await?;

                    return Ok(());
                }
            }
        }

        anyhow::bail!("no scripts were found for command: {}", name)
    }

    pub async fn execute_command(
        command: &str,
        project_path: &Path,
        ctx: &Context,
    ) -> anyhow::Result<()> {
        if let Some(scripts_ctx) = &ctx.project.scripts {
            if let Some(script_ctx) = scripts_ctx.items.get(command) {
                ScriptExecutor::new(project_path.into(), ctx.clone())
                    .run(script_ctx, command)
                    .await?;

                return Ok(());
            }
        }

        if let Some(plan) = &ctx.plan {
            if let Some(scripts_ctx) = &plan.scripts {
                if let Some(script_ctx) = scripts_ctx.items.get(command) {
                    ScriptExecutor::new(project_path.into(), ctx.clone())
                        .run(script_ctx, command)
                        .await?;

                    return Ok(());
                }
            }
        }

        anyhow::bail!("no scripts were found for command: {}", command)
    }

    pub async fn execute_command_if_exists(
        command: &str,
        project_path: &Path,
        ctx: &Context,
    ) -> anyhow::Result<()> {
        if let Some(scripts_ctx) = &ctx.project.scripts {
            if let Some(script_ctx) = scripts_ctx.items.get(command) {
                ScriptExecutor::new(project_path.into(), ctx.clone())
                    .run(script_ctx, command)
                    .await?;

                return Ok(());
            }
        }

        if let Some(plan) = &ctx.plan {
            if let Some(scripts_ctx) = &plan.scripts {
                if let Some(script_ctx) = scripts_ctx.items.get(command) {
                    ScriptExecutor::new(project_path.into(), ctx.clone())
                        .run(script_ctx, command)
                        .await?;

                    return Ok(());
                }
            }
        }

        Ok(())
    }
}
