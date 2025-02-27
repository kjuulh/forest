use std::{collections::BTreeMap, path::Path};

use crate::{model::Context, script::ScriptExecutor};

// Run is a bit special in that because the arguments dynamically render, we need to do some special magic in
// clap to avoid having to do hacks to register clap subcommands midcommand. As such instead, we opt to simply
// create a new sub command that encapsulates all the run complexities
pub struct Run {}
impl Run {
    pub fn augment_command(root: clap::Command, ctx: &Context) -> clap::Command {
        let mut run_cmd = clap::Command::new("run")
            .subcommand_required(true)
            .about("runs any kind of script from either the project or plan");

        if let Some(scripts) = &ctx.project.scripts {
            for name in scripts.items.keys() {
                let cmd = clap::Command::new(name.to_string());
                run_cmd = run_cmd.subcommand(cmd);
            }
        }

        root.subcommand(run_cmd)
    }

    pub async fn execute(
        args: &clap::ArgMatches,
        project_path: &Path,
        ctx: &Context,
    ) -> anyhow::Result<()> {
        let Some((name, args)) = args.subcommand() else {
            anyhow::bail!("failed to find a matching run command")
        };

        let scripts_ctx = ctx.project.scripts.as_ref().expect("to find scripts");
        let Some(script_ctx) = scripts_ctx.items.get(name) else {
            anyhow::bail!("failed to find script: {}", name);
        };

        ScriptExecutor::new(project_path.into(), ctx.clone())
            .run(script_ctx, name)
            .await?;

        Ok(())
    }
}
