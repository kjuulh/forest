use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;

use crate::{grpc::GrpcClientState, models::source::Source, state::State};

#[derive(clap::Parser)]
pub struct PublishCommand {
    #[arg(long)]
    metadata: Vec<String>,

    #[arg(long = "source-username")]
    source_username: Option<String>,

    #[arg(long = "source-email")]
    source_email: Option<String>,

    #[arg(long = "context-title")]
    context_title: String,

    #[arg(long = "context-description")]
    context_description: Option<String>,

    #[arg(long = "context-web")]
    context_web: Option<String>,

    #[arg(long = "project-namespace")]
    project_namespace: String,

    #[arg(long = "project-name")]
    project_name: String,

    #[arg(long = "commit-sha")]
    commit_sha: Option<String>,

    #[arg(long = "commit-branch")]
    commit_branch: Option<String>,
}

static LARGE_PAYOAD: [u8; 4000000] = [b'a'; 4000000];

impl PublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let grpc = state.grpc_client();

        let upload_handle = grpc
            .begin_artifact_upload()
            .await
            .context("begin artifact upload")?;

        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(".non/deployment") {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;

            if !metadata.is_file() {
                continue;
            }

            files.push(path.to_path_buf());
        }

        for file in files {
            let artifact_file = file.strip_prefix(".non/deployment")?;
            let mut components = artifact_file.components();
            let Some(env) = components.next() else {
                tracing::warn!("file doesn't exist, env is required");
                continue;
            };
            let Some(destination) = components.next() else {
                tracing::warn!("file doesn't exist, destination is required");
                continue;
            };

            let destination = destination.as_os_str().to_string_lossy();
            let destination = destination.replace(".", "/");

            let Some(_destination_type_namespace) = components.next() else {
                tracing::warn!("file doesn't exist, destination_type_namespace is required");
                continue;
            };
            let Some(_destination_type_name) = components.next() else {
                tracing::warn!("file doesn't exist, destination_type_name is required");
                continue;
            };

            let _file_name = components.collect::<PathBuf>();
            let file_content = tokio::fs::read_to_string(&file)
                .await
                .context("failed to read template file")?;

            let file_path = artifact_file.to_string_lossy();
            tracing::info!("uploading file: {}", file_path);
            grpc.upload_artifact_file(
                &upload_handle,
                &file_path,
                &file_content,
                &env.as_os_str().to_string_lossy(),
                &destination,
            )
            .await
            .context("upload file")?;
        }

        // let large_payload = str::from_utf8(&LARGE_PAYOAD).unwrap();

        // for i in 0..9 {
        //     tracing::info!("uploading file: {}", i);
        //     grpc.upload_artifact_file(
        //         &upload_handle,
        //         &i.to_string(),
        //         large_payload,
        //         "some-env",
        //         "some-dest",
        //     )
        //     .await
        //     .context("upload first file")?;
        // }

        let artifact_id = grpc
            .commit_artifact_upload(upload_handle)
            .await
            .context("commit artifact upload")?;

        let metadata = self
            .metadata
            .iter()
            .map(|m| {
                m.split_once("=")
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect::<Option<HashMap<String, String>>>()
            .ok_or(anyhow::anyhow!("meta data item did not contain a '='"))?;

        let source = Source {
            username: self.source_username.clone(),
            email: self.source_email.clone(),
        };
        let context = crate::models::context::ArtifactContext {
            title: self.context_title.clone(),
            description: self.context_description.clone(),
            web: self.context_web.clone(),
        };
        let project = crate::models::project::Project {
            namespace: self.project_namespace.clone(),
            project: self.project_name.clone(),
        };

        let reference = crate::models::reference::Reference {
            commit_sha: self
                .commit_sha
                .clone()
                .context("commit sha not found : (TODO get from context)")?,
            commit_branch: self.commit_branch.clone(),
        };

        let slug = grpc
            .annotate_artifact(
                &artifact_id,
                &metadata,
                &source,
                &context,
                &project,
                &reference,
            )
            .await
            .context("annotate artifact")?;

        println!("published artifact: {slug}\n");
        println!("$ non project release {slug} --destination <prod/k8s/eu-west-1/001>");

        Ok(())
    }
}
