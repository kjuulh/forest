use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use kdl::{KdlDocument, KdlNode, KdlValue};
use rusty_s3::{Bucket, Credentials, S3Action};

use crate::state::SharedState;

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

#[derive(Debug, Clone)]
pub enum ProjectPlan {
    Local { path: PathBuf },
    NoPlan,
}

impl TryFrom<&KdlNode> for ProjectPlan {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        let Some(children) = value.children() else {
            return Ok(Self::NoPlan);
        };

        if let Some(local) = children.get_arg("local") {
            return Ok(Self::Local {
                path: local
                    .as_string()
                    .map(|l| l.to_string())
                    .ok_or(anyhow::anyhow!("local must have an arg with a valid path"))?
                    .into(),
            });
        }

        Ok(Self::NoPlan)
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    name: String,
    description: Option<String>,
    plan: Option<ProjectPlan>,
}

impl TryFrom<KdlDocument> for Project {
    type Error = anyhow::Error;

    fn try_from(value: KdlDocument) -> Result<Self, Self::Error> {
        let project_section = value.get("project").ok_or(anyhow::anyhow!(
            "forest.kdl project file must have a project object"
        ))?;

        let project_children = project_section
            .children()
            .ok_or(anyhow::anyhow!("a forest project must have children"))?;

        let project_plan: Option<ProjectPlan> = if let Some(project) = project_children.get("plan")
        {
            Some(project.try_into()?)
        } else {
            None
        };

        Ok(Self {
            name: project_children
                .get_arg("name")
                .and_then(|n| match n {
                    KdlValue::String(s) => Some(s),
                    _ => None,
                })
                .cloned()
                .ok_or(anyhow::anyhow!("a forest kuddle project must have a name"))?,
            description: project_children
                .get_arg("description")
                .and_then(|n| match n {
                    KdlValue::String(s) => Some(s.trim().to_string()),
                    _ => None,
                }),
            plan: project_plan,
        })
    }
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

            let project_file = tokio::fs::read_to_string(project_file_path).await?;
            let project_doc: KdlDocument = project_file.parse()?;

            let project: Project = project_doc.try_into()?;

            tracing::trace!("found a project name: {}, {:?}", project.name, project);
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
        _ => (),
    }

    Ok(())
}
