use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use kdl::KdlDocument;
use rusty_s3::{Bucket, Credentials, S3Action};
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, ThemeSet},
    parsing::SyntaxSet,
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};
use syntect_assets::assets::HighlightingAssets;
use tokio::io::AsyncWriteExt;

use crate::{
    model::{Context, Plan, Project, TemplateType},
    plan_reconciler::PlanReconciler,
    state::SharedState,
};

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(
        env = "FOREST_PROJECT_PATH",
        long = "project-path",
        default_value = "."
    )]
    project_path: PathBuf,
}

#[derive(Subcommand)]
enum Commands {
    Init {},

    Template {},

    Info {},

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

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();

    let project_path = &cli.project_path.canonicalize()?;
    let project_file_path = project_path.join("forest.kdl");
    if !project_file_path.exists() {
        anyhow::bail!(
            "no 'forest.kdl' file was found at: {}",
            project_file_path.display().to_string()
        );
    }

    let project_file = tokio::fs::read_to_string(&project_file_path).await?;
    let project_doc: KdlDocument = project_file.parse()?;

    let project: Project = project_doc.try_into()?;
    tracing::trace!("found a project name: {}", project.name);

    let plan = if let Some(plan_file_path) = PlanReconciler::new()
        .reconcile(&project, project_path)
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

    match cli.command.unwrap() {
        Commands::Init {} => {
            tracing::info!("initializing project");
            tracing::trace!("found context: {:?}", context);
        }

        Commands::Info {} => {
            let output = serde_json::to_string_pretty(&context)?;
            let assets = HighlightingAssets::from_binary();
            let theme = assets.get_theme("OneHalfDark");

            let ss = SyntaxSet::load_defaults_nonewlines();

            let syntax = ss.find_syntax_by_extension("json").unwrap();
            let mut h = HighlightLines::new(syntax, theme);

            for line in LinesWithEndings::from(&output) {
                let ranges: Vec<(Style, &str)> = h.highlight_line(line, &ss).unwrap();
                print!("{}", as_24_bit_terminal_escaped(&ranges[..], true));
            }
            println!()
        }

        Commands::Template {} => {
            tracing::info!("templating");

            let Some(template) = context.project.templates else {
                return Ok(());
            };

            match template.ty {
                TemplateType::Jinja2 => {
                    for entry in glob::glob(&format!(
                        "{}/{}",
                        project_path.display().to_string().trim_end_matches("/"),
                        template.path.trim_start_matches("./"),
                    ))
                    .map_err(|e| anyhow::anyhow!("failed to read glob pattern: {}", e))?
                    {
                        let entry =
                            entry.map_err(|e| anyhow::anyhow!("failed to read path: {}", e))?;
                        let entry_name = entry.display().to_string();

                        let entry_rel = if entry.is_absolute() {
                            entry.strip_prefix(project_path).map(|e| e.to_path_buf())
                        } else {
                            Ok(entry.clone())
                        };

                        let rel_file_path = entry_rel
                            .map(|p| {
                                if p.file_name()
                                    .map(|f| f.to_string_lossy().ends_with(".jinja2"))
                                    .unwrap_or(false)
                                {
                                    p.with_file_name(
                                        p.file_stem().expect("to be able to find a filename"),
                                    )
                                } else {
                                    p.to_path_buf()
                                }
                            })
                            .map_err(|e| {
                                anyhow::anyhow!(
                                    "failed to find relative file: {}, project: {}, file: {}",
                                    e,
                                    project_path.display(),
                                    entry_name
                                )
                            })?;

                        let output_file_path = project_path
                            .join(".forest/temp")
                            .join(&template.output)
                            .join(rel_file_path);

                        let contents = tokio::fs::read_to_string(&entry).await.map_err(|e| {
                            anyhow::anyhow!(
                                "failed to read template: {}, err: {}",
                                entry.display(),
                                e
                            )
                        })?;

                        let mut env = minijinja::Environment::new();
                        env.add_template(&entry_name, &contents)?;
                        let tmpl = env.get_template(&entry_name)?;

                        let output = tmpl
                            .render(minijinja::context! {})
                            .map_err(|e| anyhow::anyhow!("failed to render template: {}", e))?;

                        tracing::info!("rendered template: {}", output);

                        if let Some(parent) = output_file_path.parent() {
                            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                                anyhow::anyhow!(
                                    "failed to create directory (path: {}) for output: {}",
                                    parent.display(),
                                    e
                                )
                            })?;
                        }

                        let mut output_file = tokio::fs::File::create(&output_file_path)
                            .await
                            .map_err(|e| {
                                anyhow::anyhow!(
                                    "failed to create file: {}, error: {}",
                                    output_file_path.display(),
                                    e
                                )
                            })?;
                        output_file.write_all(output.as_bytes()).await?;
                    }
                }
            }
        }

        Commands::Serve {
            host,
            s3_endpoint,
            s3_bucket,
            s3_region,
            s3_user,
            s3_password,
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
    }

    Ok(())
}
