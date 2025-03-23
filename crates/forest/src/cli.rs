use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context as AnyContext;
use clap::{FromArgMatches, Parser, Subcommand, crate_authors, crate_description, crate_version};
use colored_json::ToColoredJson;
use kdl::KdlDocument;
use rusty_s3::{Bucket, Credentials, S3Action};

use crate::{
    model::{Context, ForestFile, Plan, Project, WorkspaceProject},
    plan_reconciler::PlanReconciler,
    state::SharedState,
};

mod run;
mod template;

#[derive(Subcommand)]
enum Commands {
    Init {},
    Template(template::Template),
    Info {},
    Clean {},
    Serve {
        #[arg(env = "FOREST_HOST", long, default_value = "127.0.0.1:3000")]
        host: SocketAddr,

        #[arg(env = "FOREST_S3_ENDPOINT", long = "s3-endpoint")]
        s3_endpoint: String,

        #[arg(env = "FOREST_S3_REGION", long = "s3-region")]
        s3_region: String,

        #[arg(env = "FOREST_S3_BUCKET", long = "s3-bucket")]
        s3_bucket: String,

        #[arg(env = "FOREST_S3_USER", long = "s3-user")]
        s3_user: String,

        #[arg(env = "FOREST_S3_PASSWORD", long = "s3-password")]
        s3_password: String,
    },
}

fn get_root(include_run: bool) -> clap::Command {
    let mut root_cmd = clap::Command::new("forest")
        .subcommand_required(true)
        .author(crate_authors!())
        .version(crate_version!())
        .about(crate_description!())
        .ignore_errors(include_run)
        .arg(
            clap::Arg::new("project_path")
                .long("project-path")
                .env("FOREST_PROJECT_PATH")
                .default_value("."),
        );

    if include_run {
        root_cmd = root_cmd.subcommand(clap::Command::new("run").allow_external_subcommands(true))
    }
    Commands::augment_subcommands(root_cmd)
}

pub async fn execute() -> anyhow::Result<()> {
    let matches = get_root(true).get_matches();
    let project_path = PathBuf::from(
        &matches
            .get_one::<String>("project_path")
            .expect("project path always to be set"),
    )
    .canonicalize()?;
    let project_file_path = project_path.join("forest.kdl");
    if !project_file_path.exists() {
        anyhow::bail!(
            "no 'forest.kdl' file was found at: {}",
            project_file_path.display().to_string()
        );
    }

    let project_file = tokio::fs::read_to_string(&project_file_path).await?;
    let doc: KdlDocument = project_file.parse()?;
    let project: ForestFile = doc.try_into()?;

    match project {
        ForestFile::Workspace(workspace) => {
            tracing::trace!("running as workspace");

            // 1. For each member load the project

            let mut workspace_members = Vec::new();

            for member in workspace.members {
                let workspace_member_path = project_path.join(&member.path);

                let project_file_path = workspace_member_path.join("forest.kdl");
                if !project_file_path.exists() {
                    anyhow::bail!(
                        "no 'forest.kdl' file was found at: {}",
                        workspace_member_path.display().to_string()
                    );
                }

                let project_file = tokio::fs::read_to_string(&project_file_path).await?;
                let doc: KdlDocument = project_file.parse()?;
                let project: WorkspaceProject = doc.try_into().context(format!(
                    "workspace member: {} failed to parse",
                    &member.path
                ))?;

                workspace_members.push((workspace_member_path, project));
            }

            // TODO: 1a (optional). Resolve dependencies
            // 2. Reconcile plans

            let mut member_contexts = Vec::new();

            for (member_path, member) in workspace_members {
                match member {
                    WorkspaceProject::Plan(plan) => {
                        tracing::warn!("skipping reconcile for plans for now")
                    }
                    WorkspaceProject::Project(project) => {
                        let plan = if let Some(plan_file_path) = PlanReconciler::new()
                            .reconcile(&project, &project_path)
                            .await?
                        {
                            let plan_file = tokio::fs::read_to_string(&plan_file_path).await?;
                            let plan_doc: KdlDocument = plan_file.parse()?;

                            let plan: Plan = plan_doc.try_into()?;
                            tracing::trace!("found a plan name: {}", project.name);

                            Some(plan)
                        } else {
                            None
                        };

                        let context = Context { project, plan };
                        member_contexts.push((member_path, context));
                    }
                }
            }

            tracing::debug!("run is called, building extra commands, rerunning the parser");
            let mut run_cmd = clap::Command::new("run").subcommand_required(true);

            // 3. Provide context and aggregated commands for projects
            for (_, context) in &member_contexts {
                let commands = run::Run::augment_workspace_command(context, &context.project.name);
                run_cmd = run_cmd.subcommands(commands);
            }

            run_cmd =
                run_cmd.subcommand(clap::Command::new("all").allow_external_subcommands(true));

            let mut root = get_root(false).subcommand(run_cmd);
            let matches = root.get_matches_mut();

            if matches.subcommand().is_none() {
                root.print_help()?;
                anyhow::bail!("failed to find command");
            }

            match matches
                .subcommand()
                .expect("forest requires a command to be passed")
            {
                ("run", args) => {
                    let (run_args, args) = args.subcommand().expect("run must have subcommands");

                    match run_args {
                        "all" => {
                            let (all_cmd, _args) = args
                                .subcommand()
                                .expect("to be able to get a subcommand (todo: might not work)");

                            for (member_path, context) in member_contexts {
                                run::Run::execute_command_if_exists(
                                    all_cmd,
                                    &member_path,
                                    &context,
                                )
                                .await?;
                            }
                        }
                        _ => {
                            let (project_name, command) = run_args
                                .split_once("::")
                                .expect("commands to always be pairs for workspaces");

                            let mut found_context = false;
                            for (member_path, context) in &member_contexts {
                                if project_name == context.project.name {
                                    run::Run::execute_command(command, member_path, context)
                                        .await?;

                                    found_context = true;
                                }
                            }

                            if !found_context {
                                anyhow::bail!("no matching context was found")
                            }
                        }
                    }
                }
                _ => match Commands::from_arg_matches(&matches).unwrap() {
                    Commands::Init {} => {
                        tracing::info!("initializing project");
                    }
                    Commands::Info {} => {
                        let output = serde_json::to_string_pretty(&member_contexts)?;
                        println!("{}", output.to_colored_json_auto().unwrap_or(output));
                    }
                    Commands::Template(template) => {
                        //template.execute(&project_path, &context).await?;
                    }
                    Commands::Serve {
                        s3_endpoint,
                        s3_bucket,
                        s3_region,
                        s3_user,
                        s3_password,
                        ..
                    } => {
                        tracing::info!("Starting server");
                        let creds = Credentials::new(s3_user, s3_password);
                        let bucket = Bucket::new(
                            url::Url::parse(&s3_endpoint)?,
                            rusty_s3::UrlStyle::Path,
                            s3_bucket,
                            s3_region,
                        )?;
                        let put_object = bucket.put_object(Some(&creds), "some-object");
                        let _url = put_object.sign(std::time::Duration::from_secs(30));
                        let _state = SharedState::new().await?;
                    }
                    Commands::Clean {} => {
                        todo!();
                        // let forest_path = project_path.join(".forest");
                        // if forest_path.exists() {
                        //     tokio::fs::remove_dir_all(forest_path).await?;
                        //     tracing::info!("removed .forest");
                        // }
                    }
                },
            }
        }
        ForestFile::Project(project) => {
            tracing::trace!("found a project name: {}", project.name);

            let plan = if let Some(plan_file_path) = PlanReconciler::new()
                .reconcile(&project, &project_path)
                .await?
            {
                let plan_file = tokio::fs::read_to_string(&plan_file_path).await?;
                let plan_doc: KdlDocument = plan_file.parse()?;

                let plan: Plan = plan_doc.try_into()?;
                tracing::trace!("found a plan name: {}", project.name);

                Some(plan)
            } else {
                None
            };

            let context = Context { project, plan };

            let matches = if matches.subcommand_matches("run").is_some() {
                tracing::debug!("run is called, building extra commands, rerunning the parser");
                let root = get_root(false);

                let run_cmd = run::Run::augment_command(&context);
                root.subcommand(run_cmd).get_matches()
            } else {
                matches
            };

            match matches
                .subcommand()
                .expect("forest requires a command to be passed")
            {
                ("run", args) => {
                    run::Run::execute(args, &project_path, &context).await?;
                }
                _ => match Commands::from_arg_matches(&matches).unwrap() {
                    Commands::Init {} => {
                        tracing::info!("initializing project");
                        tracing::trace!("found context: {:?}", context);
                    }
                    Commands::Info {} => {
                        let output = serde_json::to_string_pretty(&context)?;
                        println!("{}", output.to_colored_json_auto().unwrap_or(output));
                    }
                    Commands::Template(template) => {
                        template.execute(&project_path, &context).await?;
                    }
                    Commands::Serve {
                        s3_endpoint,
                        s3_bucket,
                        s3_region,
                        s3_user,
                        s3_password,
                        ..
                    } => {
                        tracing::info!("Starting server");
                        let creds = Credentials::new(s3_user, s3_password);
                        let bucket = Bucket::new(
                            url::Url::parse(&s3_endpoint)?,
                            rusty_s3::UrlStyle::Path,
                            s3_bucket,
                            s3_region,
                        )?;
                        let put_object = bucket.put_object(Some(&creds), "some-object");
                        let _url = put_object.sign(std::time::Duration::from_secs(30));
                        let _state = SharedState::new().await?;
                    }
                    Commands::Clean {} => {
                        let forest_path = project_path.join(".forest");
                        if forest_path.exists() {
                            tokio::fs::remove_dir_all(forest_path).await?;
                            tracing::info!("removed .forest");
                        }
                    }
                },
            }
        }
    }

    Ok(())
}
