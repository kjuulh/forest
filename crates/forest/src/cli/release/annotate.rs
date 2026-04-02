use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;

use crate::{grpc::GrpcClientState, models::source::Source, state::State};

/// Run a git command and return its trimmed stdout, or `None` on failure.
pub(super) async fn git_output(args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[derive(clap::Parser)]
pub struct AnnotateCommand {
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

    /// Skip automatic trigger evaluation (no auto-release from policies).
    #[arg(long = "annotation-only")]
    annotation_only: bool,
}

impl AnnotateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let slug = annotate(
            state,
            &AnnotateParams {
                metadata: self.metadata.clone(),
                source_username: self.source_username.clone(),
                source_email: self.source_email.clone(),
                context_title: self.context_title.clone(),
                context_description: self.context_description.clone(),
                context_web: self.context_web.clone(),
                organisation: self.organisation.clone(),
                project_name: self.project_name.clone(),
                commit_sha: self.commit_sha.clone(),
                commit_branch: self.commit_branch.clone(),
                source_type: self.source_type.clone(),
                run_url: self.run_url.clone(),
                context_pr: self.context_pr.clone(),
                commit_message: self.commit_message.clone(),
                version: self.version.clone(),
                repo_url: self.repo_url.clone(),
                spec_file: self.spec_file.clone(),
                no_spec: self.no_spec,
                include_files: self.include_files.clone(),
                annotation_only: self.annotation_only,
            },
        )
        .await?;

        println!("published artifact: {slug}\n");
        println!("$ forest release {slug} --destination <prod/k8s/eu-west-1/001>");

        Ok(())
    }
}

/// Parameters for the annotate operation, shared between AnnotateCommand and CreateCommand.
pub struct AnnotateParams {
    pub metadata: Vec<String>,
    pub source_username: Option<String>,
    pub source_email: Option<String>,
    pub context_title: String,
    pub context_description: Option<String>,
    pub context_web: Option<String>,
    pub organisation: String,
    pub project_name: String,
    pub commit_sha: Option<String>,
    pub commit_branch: Option<String>,
    pub source_type: Option<String>,
    pub run_url: Option<String>,
    pub context_pr: Option<String>,
    pub commit_message: Option<String>,
    pub version: Option<String>,
    pub repo_url: Option<String>,
    pub spec_file: Option<String>,
    pub no_spec: bool,
    pub include_files: Vec<String>,
    pub annotation_only: bool,
}

/// Core annotate logic. Returns the artifact slug on success.
pub async fn annotate(state: &State, params: &AnnotateParams) -> anyhow::Result<String> {
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
    if !params.no_spec {
        let spec_path = if let Some(ref spec) = params.spec_file {
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
    for include_path_str in &params.include_files {
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

    let metadata = params
        .metadata
        .iter()
        .map(|m| {
            m.split_once("=")
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect::<Option<HashMap<String, String>>>()
        .ok_or(anyhow::anyhow!("meta data item did not contain a '='"))?;

    let source = Source {
        username: params.source_username.clone(),
        email: params.source_email.clone(),
        user_id: None, // set server-side from auth token
        source_type: params.source_type.clone(),
        run_url: params.run_url.clone(),
    };
    let context = crate::models::context::ArtifactContext {
        title: params.context_title.clone(),
        description: params.context_description.clone(),
        web: params.context_web.clone(),
        pr: params.context_pr.clone(),
    };
    let project = crate::models::project::Project {
        organisation: params.organisation.clone(),
        project: params.project_name.clone(),
    };

    let commit_sha = match params.commit_sha.clone() {
        Some(sha) => sha,
        None => {
            git_output(&["rev-parse", "HEAD"])
                .await
                .context("--commit-sha is required (not in a git repository, or git not found)")?
        }
    };

    let commit_branch = match params.commit_branch.clone() {
        Some(branch) => Some(branch),
        None => git_output(&["branch", "--show-current"]).await,
    };

    let reference = crate::models::reference::Reference {
        commit_sha,
        commit_branch,
        commit_message: params.commit_message.clone(),
        version: params.version.clone(),
        repo_url: params.repo_url.clone(),
    };

    let slug = grpc
        .annotate_artifact(
            &artifact_id,
            &metadata,
            &source,
            &context,
            &project,
            &reference,
            params.annotation_only,
        )
        .await
        .context("annotate artifact")?;

    Ok(slug)
}
