use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;

use crate::{grpc::GrpcClientState, models::source::Source, state::State};

#[derive(clap::Parser)]
pub struct PublishCommand {
    #[arg(long)]
    metadata: Vec<String>,

    /// Source username (only used by app tokens; ignored for user tokens)
    #[arg(long = "source-username")]
    source_username: Option<String>,

    /// Source email (only used by app tokens; ignored for user tokens)
    #[arg(long = "source-email")]
    source_email: Option<String>,

    #[arg(long = "context-title")]
    context_title: String,

    #[arg(long = "context-description")]
    context_description: Option<String>,

    #[arg(long = "context-web")]
    context_web: Option<String>,

    #[arg(long, short = 'o')]
    organisation: String,

    #[arg(long = "project-name")]
    project_name: String,

    #[arg(long = "commit-sha")]
    commit_sha: Option<String>,

    #[arg(long = "commit-branch")]
    commit_branch: Option<String>,

    #[arg(long = "source-type")]
    source_type: Option<String>,

    #[arg(long = "run-url")]
    run_url: Option<String>,

    #[arg(long = "context-pr")]
    context_pr: Option<String>,

    #[arg(long = "commit-message")]
    commit_message: Option<String>,

    #[arg(long)]
    version: Option<String>,

    #[arg(long = "repo-url")]
    repo_url: Option<String>,

    /// Path to the spec file (e.g. forest.cue). Auto-detected from cwd if not specified.
    #[arg(long = "spec-file")]
    spec_file: Option<String>,

    /// Skip uploading the spec file even if one is found.
    #[arg(long = "no-spec")]
    no_spec: bool,

    /// Additional files to include as attachments. Can be specified multiple times.
    #[arg(long = "include-file")]
    include_files: Vec<String>,
}

impl PublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let grpc = state.grpc_client();

        let upload_handle = grpc
            .begin_artifact_upload()
            .await
            .context("begin artifact upload")?;

        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(".forest/deployment") {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;

            if !metadata.is_file() {
                continue;
            }

            files.push(path.to_path_buf());
        }

        for file in files {
            let artifact_file = file.strip_prefix(".forest/deployment")?;
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
                "deployment",
            )
            .await
            .context("upload file")?;
        }

        // Upload spec file
        if !self.no_spec {
            let spec_path = if let Some(ref spec) = self.spec_file {
                let p = std::path::PathBuf::from(spec);
                if p.exists() {
                    Some(p)
                } else {
                    anyhow::bail!("specified spec file does not exist: {}", spec);
                }
            } else {
                ["forest.cue", "forest.toml", "forest.ncl", "forest.yaml"]
                    .iter()
                    .map(std::path::PathBuf::from)
                    .find(|p| p.exists())
            };

            if let Some(spec_path) = spec_path {
                let file_name = spec_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "forest.cue".to_string());
                let file_content = tokio::fs::read_to_string(&spec_path)
                    .await
                    .context(format!("failed to read spec file: {}", spec_path.display()))?;

                tracing::info!("uploading spec file: {}", file_name);
                grpc.upload_artifact_file(
                    &upload_handle,
                    &file_name,
                    &file_content,
                    "",
                    "",
                    "spec",
                )
                .await
                .context("upload spec file")?;
            } else {
                tracing::debug!("no spec file found, skipping spec upload");
            }
        }

        // Upload additional include files
        for include_path_str in &self.include_files {
            let include_path = std::path::PathBuf::from(include_path_str);
            if !include_path.exists() {
                anyhow::bail!("include file does not exist: {}", include_path_str);
            }

            let file_name = include_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| include_path_str.clone());
            let file_content = tokio::fs::read_to_string(&include_path)
                .await
                .context(format!(
                    "failed to read include file: {}",
                    include_path.display()
                ))?;

            tracing::info!("uploading attachment: {}", file_name);
            grpc.upload_artifact_file(
                &upload_handle,
                &file_name,
                &file_content,
                "",
                "",
                "attachment",
            )
            .await
            .context(format!("upload include file: {}", include_path_str))?;
        }

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
            user_id: None, // set server-side from auth token
            source_type: self.source_type.clone(),
            run_url: self.run_url.clone(),
        };
        let context = crate::models::context::ArtifactContext {
            title: self.context_title.clone(),
            description: self.context_description.clone(),
            web: self.context_web.clone(),
            pr: self.context_pr.clone(),
        };
        let project = crate::models::project::Project {
            organisation: self.organisation.clone(),
            project: self.project_name.clone(),
        };

        let reference = crate::models::reference::Reference {
            commit_sha: self
                .commit_sha
                .clone()
                .context("commit sha not found : (TODO get from context)")?,
            commit_branch: self.commit_branch.clone(),
            commit_message: self.commit_message.clone(),
            version: self.version.clone(),
            repo_url: self.repo_url.clone(),
        };

        let slug = grpc
            .annotate_artifact(
                &artifact_id,
                &metadata,
                &source,
                &context,
                &project,
                &reference,
                false,
            )
            .await
            .context("annotate artifact")?;

        // Slug to stdout so `slug=$(forest project publish)` works; the
        // next-step hint goes to stderr so it doesn't pollute the variable.
        eprintln!("published artifact: {slug}");
        eprintln!();
        eprintln!("Next step:");
        eprintln!("  $ forest project release {slug} --destination <prod/k8s/eu-west-1/001>");
        println!("{slug}");

        Ok(())
    }
}
