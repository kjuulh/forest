use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use kdl::KdlDocument;
use rusty_s3::{Bucket, Credentials, S3Action};

use crate::{
    model::{Context, Plan, Project},
    plan_reconciler::PlanReconciler,
    state::SharedState,
};

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(
            env = "FOREST_PROJECT_PATH",
            long = "project-path",
            default_value = "."
        )]
        project_path: PathBuf,
    },

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

    match cli.command.unwrap() {
        Commands::Init { project_path } => {
            tracing::info!("initializing project");

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

            tracing::info!("context: {:+?}", context);
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
