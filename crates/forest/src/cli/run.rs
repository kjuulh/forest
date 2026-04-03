use std::process::Stdio;
use std::sync::Arc;

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
        if self.args.is_empty() {
            let (_, mut run_cmd) = build_dynamic_command(&project);
            let help = run_cmd.render_long_help();
            println!("{help}");
            return Ok(());
        }

        if self.args.iter().any(|a| a == "--help" || a == "-h")
            || self.args.first().map(|a| a.as_str()) == Some("help")
        {
            let (cli_names, run_cmd) = build_dynamic_command(&project);
            // Find the first non-help arg to see if user is asking for help on a specific subcommand
            let subcmd_name = self
                .args
                .iter()
                .find(|a| *a != "--help" && *a != "-h" && *a != "help");
            if let Some(name) = subcmd_name {
                if cli_names.contains_key(name.as_str()) {
                    let mut sub = build_enriched_subcommand(name, &cli_names, &project).await;
                    let help = sub.render_long_help();
                    println!("{help}");
                    return Ok(());
                }
            }
            let mut run_cmd = run_cmd;
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

/// Build an enriched clap subcommand for help display.
///
/// Fetches the component's OpenAPI spec from `forest.component.cue` to discover
/// input fields, then adds them as proper `--arg` definitions so that `--help`
/// shows meaningful argument documentation.
async fn build_enriched_subcommand(
    cli_name: &str,
    cli_names: &std::collections::BTreeMap<String, crate::models::CommandName>,
    project: &Project,
) -> clap::Command {
    let cmd_name = &cli_names[cli_name];
    let mut sub = clap::Command::new(cli_name.to_string());

    // Add description
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

    // Try to fetch input args from the component's OpenAPI spec
    let command_short = cmd_name.command_name();
    if let Some(fields) = fetch_command_input_schema(cmd_name, command_short).await {
        for field in &fields {
            let mut arg = clap::Arg::new(field.name.clone())
                .long(field.name.clone())
                .required(field.required)
                .value_name(
                    field.field_type.as_deref().unwrap_or("string").to_uppercase(),
                );
            if let Some(desc) = &field.description {
                arg = arg.help(desc.to_string());
            }
            sub = sub.arg(arg);
        }
    } else {
        // Fallback: keep the generic trailing args
        sub = sub.arg(
            clap::Arg::new("input_args")
                .action(clap::ArgAction::Append)
                .allow_hyphen_values(true)
                .trailing_var_arg(true),
        );
    }

    sub
}

struct InputField {
    name: String,
    required: bool,
    description: Option<String>,
    field_type: Option<String>,
}

/// Fetch the input fields for a command from the component's OpenAPI spec.
///
/// Runs `cue def --out openapi` on the component's `forest.component.cue`,
/// then extracts the input properties for the given command name.
async fn fetch_command_input_schema(
    cmd_name: &crate::models::CommandName,
    command_short: &str,
) -> Option<Vec<InputField>> {
    let component_dir = match cmd_name {
        crate::models::CommandName::Component {
            source: crate::models::CommandSource::Local(path),
            ..
        } => path.clone(),
        _ => return None,
    };

    let component_cue = component_dir.join("forest.component.cue");
    if !component_cue.exists() {
        return None;
    }

    let mut cmd = tokio::process::Command::new("cue");
    if let Ok(registry) = std::env::var("CUE_REGISTRY") {
        cmd.env("CUE_REGISTRY", registry);
    }
    cmd.args(["def", "./forest.component.cue", "--out", "openapi"]);
    cmd.current_dir(&component_dir);

    let output = cmd.output().await.ok()?;
    if !output.status.success() {
        return None;
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    // Navigate: components.schemas.Commands.properties.<command_short>.properties.input
    let command_schema = doc
        .get("components")?
        .get("schemas")?
        .get("Commands")?
        .get("properties")?
        .get(command_short)?;

    let input_schema = command_schema
        .get("properties")?
        .get("input")?;

    let properties = input_schema.get("properties")?.as_object()?;
    let required_fields: std::collections::HashSet<&str> = input_schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut fields = Vec::new();
    for (name, schema) in properties {
        fields.push(InputField {
            name: name.clone(),
            required: required_fields.contains(name.as_str()),
            description: schema.get("description").and_then(|d| d.as_str()).map(String::from),
            field_type: schema.get("type").and_then(|t| t.as_str()).map(String::from),
        });
    }

    // Sort so required fields come first, then alphabetical
    fields.sort_by(|a, b| b.required.cmp(&a.required).then(a.name.cmp(&b.name)));

    Some(fields)
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

/// Build a call resolver that can invoke dependency components by their ID.
/// Maps component IDs (e.g. "kjuulh/sealed-secrets") to their local paths
/// and invokes them via the Deno or binary runtime.
fn build_call_resolver(
    project: &Project,
    context: &forest_sdk::CallContext,
) -> component_deno::ComponentCallResolver {
    // Build a map of component_id → (component_dir, entrypoint)
    let mut component_map: std::collections::HashMap<String, (std::path::PathBuf, String)> =
        std::collections::HashMap::new();

    for dep in project.dependencies.get_components() {
        let component_id = format!("{}/{}", dep.organisation, dep.name);
        if let crate::models::ComponentSource::Local(path) = &dep.source {
            if component_deno::is_deno_component(path) {
                if let Some(entrypoint) = component_deno::resolve_entrypoint(path) {
                    component_map.insert(component_id, (path.clone(), entrypoint));
                }
            }
        }
    }

    let component_map = Arc::new(component_map);
    let context = context.clone();

    Box::new(move |component_id, method, spec, input, call_context| {
        let component_map = Arc::clone(&component_map);
        let base_context = context.clone();

        Box::pin(async move {
            let (component_dir, entrypoint) = component_map
                .get(&component_id)
                .ok_or_else(|| anyhow::anyhow!("unknown component: {component_id}"))?;

            // Use forwarded context if available, otherwise base context
            let ctx = call_context.unwrap_or(base_context);

            component_deno::invoke_deno_component(
                component_dir,
                entrypoint,
                &method,
                &spec,
                &input,
                Some(&ctx),
                None,
            )
            .await
        })
    })
}

/// Parse `--key value` pairs from trailing args into a JSON object.
///
/// A token is treated as a flag name only if it matches `--<letter>...` (exactly
/// two hyphens followed by an ASCII letter). This avoids misinterpreting values
/// that start with dashes (e.g. `-----BEGIN NATS USER JWT-----`).
///
/// Special value `@-` reads from stdin. Example:
///   cat creds.txt | forest run seal --value @- --key MY_KEY
///
/// Special value `@<path>` reads from a file. Example:
///   forest run seal --value @/path/to/creds.txt --key MY_KEY
async fn parse_input_args(args: &ArgMatches) -> anyhow::Result<serde_json::Value> {
    let raw: Vec<String> = args
        .get_raw("input_args")
        .unwrap_or_default()
        .map(|s| s.to_string_lossy().to_string())
        .collect();

    let mut map = serde_json::Map::new();
    let mut i = 0;
    while i < raw.len() {
        if let Some(key) = parse_flag_name(&raw[i]) {
            if i + 1 < raw.len() && parse_flag_name(&raw[i + 1]).is_none() {
                // Next token is a value (not another flag)
                let value = resolve_value(&raw[i + 1]).await?;
                map.insert(
                    key.replace('-', "_"),
                    serde_json::Value::String(value),
                );
                i += 2;
            } else {
                // No value follows — treat as boolean flag
                map.insert(key.replace('-', "_"), serde_json::Value::Bool(true));
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    Ok(serde_json::Value::Object(map))
}

/// Resolve a value that may be a literal, `@-` (stdin), or `@<path>` (file).
async fn resolve_value(raw: &str) -> anyhow::Result<String> {
    if raw == "@-" {
        use tokio::io::AsyncReadExt;
        let mut buf = String::new();
        tokio::io::stdin().read_to_string(&mut buf).await?;
        Ok(buf)
    } else if let Some(path) = raw.strip_prefix('@') {
        tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read value from file: {path}"))
    } else {
        Ok(raw.to_string())
    }
}

/// Check if a token is a CLI flag (`--name`). Returns the flag name if so.
/// Only matches exactly two hyphens followed by an ASCII letter, so values
/// like `-----BEGIN...` are not mistaken for flags.
fn parse_flag_name(token: &str) -> Option<String> {
    let rest = token.strip_prefix("--")?;
    if rest.starts_with('-') || rest.is_empty() {
        return None; // "---..." or bare "--"
    }
    if !rest.as_bytes()[0].is_ascii_alphabetic() {
        return None; // "--123" etc.
    }
    Some(rest.to_string())
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

        let input_json = parse_input_args(sub_matches).await?;

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

                let resolver = build_call_resolver(project, &call_context);

                let result = component_deno::invoke_deno_component(
                    component_dir,
                    entrypoint,
                    method,
                    &spec_json,
                    &input_json,
                    Some(&call_context),
                    Some(&resolver),
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
