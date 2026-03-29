use std::process::Stdio;

use anyhow::Context;
use clap::{Arg, ArgAction, ArgMatches, Args, FromArgMatches};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    forest_context::{ForestContext, ForestContextState},
    models::Project,
    services::{component_binary, component_deno, project::ProjectParserState},
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

        // Handle --help and help before clap parsing (since we disabled help flag
        // to allow pass-through of raw args)
        if self.args.is_empty()
            || self.args.iter().any(|a| a == "--help" || a == "-h")
            || self.args.first().map(|a| a.as_str()) == Some("help")
        {
            let (_, mut run_cmd) = build_dynamic_command(&project);
            let help = run_cmd.render_long_help();
            println!("{help}");
            return Ok(());
        }

        let (cli_names, run_cmd) = build_dynamic_command(&project);

        let cmd = clap::Command::new("forest")
            .subcommand(run_cmd)
            .disable_help_flag(true);

        let mut args = vec!["forest".to_string(), "run".to_string()];
        args.extend(self.args.iter().cloned());

        match cmd.try_get_matches_from(args) {
            Ok(matches) => {
                let (_, matches) = matches
                    .subcommand()
                    .ok_or(anyhow::anyhow!("run command is required"))?;
                CliRun.execute(&ctx, &project, matches, &cli_names).await
            }
            Err(e) => {
                match e.kind() {
                    clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion => {
                        print!("{e}");
                        Ok(())
                    }
                    clap::error::ErrorKind::MissingSubcommand => {
                        // No command given — show help
                        let (_, mut run_cmd) = build_dynamic_command(&project);
                        let help = run_cmd.render_long_help();
                        println!("{help}");
                        anyhow::bail!("no command specified")
                    }
                    _ => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}

/// Build the dynamic clap command with discovered project/component commands.
fn build_dynamic_command(
    project: &Project,
) -> (
    std::collections::BTreeMap<String, crate::models::CommandName>,
    clap::Command,
) {
    // Detect short name collisions
    let mut short_name_count: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for command_name in project.commands.keys() {
        *short_name_count
            .entry(command_name.command_name().to_string())
            .or_default() += 1;
    }

    // Build the name→key mapping
    let mut cli_names: std::collections::BTreeMap<String, crate::models::CommandName> =
        std::collections::BTreeMap::new();
    for command_name in project.commands.keys() {
        let short = command_name.command_name().to_string();
        let count = short_name_count.get(&short).copied().unwrap_or(0);

        if count > 1 {
            let qualified = command_name.to_qualified_cli_name();
            cli_names.insert(qualified, command_name.clone());
        } else {
            cli_names.insert(short, command_name.clone());
        }

        // Always register the fully qualified name as an alias
        let fqn = command_name.to_qualified_cli_name();
        if !cli_names.contains_key(&fqn) {
            cli_names.insert(fqn, command_name.clone());
        }
    }

    // Collect component names for the help footer
    let mut component_names: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for cmd_name in cli_names.values() {
        if let crate::models::CommandName::Component { name, .. } = cmd_name {
            component_names.insert(name.clone());
        }
    }

    let after_help = if !component_names.is_empty() {
        let names = component_names.into_iter().collect::<Vec<_>>().join(", ");
        format!(
            "Commands can also be invoked with qualified names:\n  \
             forest run <component>:<command>\n\n\
             Available components: {names}"
        )
    } else {
        String::new()
    };

    let mut run_cmd = clap::Command::new("run")
        .about("Run a project or component command")
        .subcommand_required(true)
        .after_help(after_help);

    for (cli_name, cmd_name) in &cli_names {
        let mut sub = clap::Command::new(cli_name.clone())
            .arg(
                clap::Arg::new("input_args")
                    .action(clap::ArgAction::Append)
                    .allow_hyphen_values(true)
                    .trailing_var_arg(true),
            );
        if let Some(command) = project.commands.get(cmd_name) {
            if let crate::models::Command::ComponentBinary {
                description: Some(desc),
                ..
            }
            | crate::models::Command::ComponentDeno {
                description: Some(desc),
                ..
            } = command
            {
                sub = sub.about(desc.clone());
            }
        }
        if cli_name.contains(':') {
            sub = sub.hide(true);
        }
        run_cmd = run_cmd.subcommand(sub);
    }

    (cli_names, run_cmd)
}

/// Build a spec JSON object from the project's component config.
fn build_spec_json(
    project: &Project,
    comp_ref: &crate::models::ComponentReference,
) -> serde_json::Value {
    match project.get_component_config(comp_ref) {
        Some(config) => serde_json::to_value(config).unwrap_or_default(),
        None => serde_json::Value::Object(serde_json::Map::new()),
    }
}

/// Parse `--key value` pairs from trailing args into a JSON object.
fn parse_input_args(args: &ArgMatches) -> serde_json::Value {
    let raw: Vec<String> = args
        .get_raw("input_args")
        .unwrap_or_default()
        .map(|s| s.to_string_lossy().to_string())
        .collect();

    let mut map = serde_json::Map::new();
    let mut i = 0;
    while i < raw.len() {
        if let Some(key) = raw[i].strip_prefix("--") {
            if i + 1 < raw.len() && !raw[i + 1].starts_with("--") {
                map.insert(key.replace('-', "_").to_string(), serde_json::Value::String(raw[i + 1].clone()));
                i += 2;
            } else {
                map.insert(key.replace('-', "_").to_string(), serde_json::Value::Bool(true));
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    serde_json::Value::Object(map)
}

struct CliRun;
impl CliRun {
    pub async fn execute(
        &self,
        ctx: &ForestContext,
        project: &Project,
        matches: &ArgMatches,
        cli_names: &std::collections::BTreeMap<String, crate::models::CommandName>,
    ) -> anyhow::Result<()> {
        let (subcommand, sub_matches) = matches
            .subcommand()
            .ok_or(anyhow::anyhow!("subcommand required"))?;

        let resolved_name = cli_names
            .get(subcommand)
            .ok_or(anyhow::anyhow!("found no matching command: {subcommand}"))?;

        let (command_name, command) = project
            .commands
            .iter()
            .find(|(c, _)| *c == resolved_name)
            .ok_or(anyhow::anyhow!("found no matching command"))?;

        let input_json = parse_input_args(sub_matches);

        tracing::info!("running command: {}", command_name);

        match command {
            crate::models::Command::Script(_) => {
                anyhow::bail!(
                    "script-based commands are not yet supported — \
                     use inline commands or v2 component binaries instead"
                )
            }
            crate::models::Command::ComponentBinary {
                binary_path,
                method,
                ..
            } => {
                let spec_json = if let Some(comp_ref) = command_name.to_component_reference() {
                    build_spec_json(project, &comp_ref)
                } else {
                    serde_json::Value::Object(serde_json::Map::new())
                };

                let call_context = forest_sdk::CallContext {
                    project: Some(project.name.clone()),
                    organisation: project.organisation.clone(),
                    work_dir: Some(project.path.to_string_lossy().to_string()),
                    ..Default::default()
                };

                let result = component_binary::invoke_component_with_context(
                    binary_path,
                    method,
                    &spec_json,
                    &input_json,
                    Some(&call_context),
                )
                .await?;

                if !result.is_null() {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
            crate::models::Command::ComponentDeno {
                component_dir,
                entrypoint,
                method,
                ..
            } => {
                let spec_json = if let Some(comp_ref) = command_name.to_component_reference() {
                    build_spec_json(project, &comp_ref)
                } else {
                    serde_json::Value::Object(serde_json::Map::new())
                };

                let call_context = forest_sdk::CallContext {
                    project: Some(project.name.clone()),
                    organisation: project.organisation.clone(),
                    work_dir: Some(project.path.to_string_lossy().to_string()),
                    ..Default::default()
                };

                let result = component_deno::invoke_deno_component(
                    component_dir,
                    entrypoint,
                    method,
                    &spec_json,
                    &input_json,
                    Some(&call_context),
                )
                .await?;

                if !result.is_null() {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
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
