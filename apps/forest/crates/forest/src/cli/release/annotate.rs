use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Serialize;
use tabled::Tabled;

use crate::{grpc::GrpcClientState, models::source::Source, state::State};

/// Where `forest release prepare` writes the manifests a release uploads.
pub(crate) const DEPLOYMENT_DIR: &str = ".forest/deployment";

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

/// Every file under `.forest/deployment`, or nothing at all when the directory
/// does not exist.
///
/// A release does not have to deploy anything. A notification-only project — an
/// empty `forest.cue` whose only job is to record that something shipped
/// elsewhere (DATA-637) — never runs `forest release prepare`, so nothing ever
/// creates `.forest/deployment`. Neither does a fresh CI checkout of a project
/// that annotates before it prepares.
///
/// `WalkDir` reports a missing root as an `Err` on the first iteration rather
/// than as an empty walk, so iterating it directly turned "there is nothing to
/// upload" into `IO error for operation on .forest/deployment: No such file or
/// directory` and killed the annotate outright. Absent means empty.
pub(crate) fn deployment_files() -> anyhow::Result<Vec<PathBuf>> {
    deployment_files_in(Path::new(DEPLOYMENT_DIR))
}

fn deployment_files_in(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !root.exists() {
        tracing::debug!("{} does not exist — nothing to upload", root.display());
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;

        if !entry.metadata()?.is_file() {
            continue;
        }

        files.push(entry.path().to_path_buf());
    }

    Ok(files)
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

    /// Organisation name. Auto-detected from the forest.cue in the working
    /// directory if not specified.
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    /// Project name. Auto-detected from the forest.cue in the working
    /// directory if not specified. `--project` is accepted too, matching
    /// `forest release create`.
    #[arg(long = "project-name", short = 'p', alias = "project")]
    project_name: Option<String>,

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

    /// Work out the release author from the surrounding environment — the
    /// GitHub Actions event payload, `GITHUB_ACTOR`, then the commit itself.
    ///
    /// Fills in `--source-username` / `--source-email` when they were not
    /// given; never overrides them. Intended for CI, where the annotation is
    /// authenticated by a shared token whose owner is not the person who wrote
    /// the change.
    #[arg(long)]
    detect: bool,
}

impl AnnotateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // Only read the spec file when something was left blank. A caller that
        // named both has no reason to need a forest.cue in the directory it
        // happens to be standing in, and `create` has auto-detected these for
        // as long as it has existed — `annotate` demanding them was the odd
        // one out, and every CI workflow spelled them out because of it.
        let (organisation, project_name) = match (
            self.organisation.clone(),
            self.project_name.clone(),
        ) {
            (Some(org), Some(project)) => (org, project),
            (org, project) => {
                let (detected_org, detected_project) = super::detect::project(state).await;

                let organisation = org.or(detected_org).context(
                        "organisation not found: set project.organisation in forest.cue or pass --organisation",
                    )?;
                let project_name = project.or(detected_project).context(
                    "project name not found: set project.name in forest.cue or pass --project-name",
                )?;

                (organisation, project_name)
            }
        };

        let annotated = annotate(
            state,
            &AnnotateParams {
                metadata: self.metadata.clone(),
                source_username: self.source_username.clone(),
                source_email: self.source_email.clone(),
                context_title: self.context_title.clone(),
                context_description: self.context_description.clone(),
                context_web: self.context_web.clone(),
                organisation,
                project_name,
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
                detect: self.detect,
            },
        )
        .await?;

        // The result goes to stdout, the narration to stderr. Everything this
        // command said used to be narration, which left `--format` with nothing
        // to apply to and scripts scraping the slug out of a prose line
        // (DATA-637). `--format name` now prints the slug alone.
        let slug = annotated.slug.clone();
        print!(
            "{}",
            crate::cli::output::render(&state.config.format, &[annotated])
        );

        eprintln!();
        eprintln!("$ forest release {slug} --destination <prod/k8s/eu-west-1/001>");

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
    /// Fill blank source fields from the environment. See `detect::resolve`.
    pub detect: bool,
}

/// Core annotate logic. Returns the artifact slug on success.
/// What an annotation produced, for callers that need to act on it.
///
/// `slug` first, deliberately: `--format name` prints the first column, so
/// `forest release annotate --format name` is the slug and nothing else, which
/// is what a CI script wants to capture (DATA-637).
#[derive(Tabled, Serialize)]
pub struct AnnotatedRelease {
    /// Human-friendly handle for the release — what `forest release <slug>` takes.
    pub slug: String,

    /// The artifact the annotation was made against.
    #[tabled(rename = "artifact id")]
    pub artifact_id: String,
}

pub async fn annotate(state: &State, params: &AnnotateParams) -> anyhow::Result<AnnotatedRelease> {
    let grpc = state.grpc_client();

    let upload_handle = grpc
        .begin_artifact_upload()
        .await
        .context("begin artifact upload")?;

    for file in deployment_files()? {
        let artifact_file = file.strip_prefix(DEPLOYMENT_DIR)?;
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
            grpc.upload_artifact_file(&upload_handle, &file_name, &file_content, "", "", "spec")
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

    let mut metadata = params
        .metadata
        .iter()
        .map(|m| {
            m.split_once("=")
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect::<Option<HashMap<String, String>>>()
        .ok_or(anyhow::anyhow!("meta data item did not contain a '='"))?;

    // Blank unless the caller said, or `--detect` found somebody. Leaving it
    // blank is what hands attribution to the token's owner, which in CI is
    // whoever created the secret rather than whoever wrote the commit.
    let attribution = super::detect::resolve(
        state,
        params.source_username.clone(),
        params.source_email.clone(),
        params.detect,
    )
    .await;

    // The raw signals ride along with the annotation. The server links them to
    // a forest user where it can and records them either way, so a commit by
    // somebody with no forest account is still attributed to a name rather than
    // falling back to the token's owner. An explicit `--metadata` on the same
    // key wins, like every other explicit flag here.
    for (key, value) in attribution.metadata {
        metadata.entry(key).or_insert(value);
    }

    let source = Source {
        username: attribution.username,
        email: attribution.email,
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
        None => git_output(&["rev-parse", "HEAD"])
            .await
            .context("--commit-sha is required (not in a git repository, or git not found)")?,
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

    Ok(AnnotatedRelease {
        slug,
        artifact_id: artifact_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract CI depends on: `--format name` prints the first column, so
    /// the slug has to be first. A script capturing `$(forest release annotate
    /// --format name)` gets the slug and nothing else — that is what replaced
    /// scraping it out of a prose line (DATA-637).
    #[test]
    fn format_name_yields_the_slug_alone() {
        let row = AnnotatedRelease {
            slug: "humbly-handsome-emu".into(),
            artifact_id: "0de05556-c9f4-4532-bc6b-5a3e29ba6a16".into(),
        };

        let out = crate::cli::output::render(&crate::cli::output::OutputFormat::Name, &[row]);

        assert_eq!(out, "humbly-handsome-emu\n");
    }

    /// And json stays machine-readable, with both fields.
    #[test]
    fn format_json_carries_slug_and_artifact_id() {
        let row = AnnotatedRelease {
            slug: "humbly-handsome-emu".into(),
            artifact_id: "0de05556-c9f4-4532-bc6b-5a3e29ba6a16".into(),
        };

        let out = crate::cli::output::render(&crate::cli::output::OutputFormat::Json, &[row]);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");

        assert_eq!(parsed[0]["slug"], "humbly-handsome-emu");
        assert_eq!(
            parsed[0]["artifact_id"],
            "0de05556-c9f4-4532-bc6b-5a3e29ba6a16"
        );
    }

    /// The regression: a project with nothing to deploy. `WalkDir` surfaces the
    /// missing root as an error on the first item, which used to abort the whole
    /// annotate (DATA-637).
    #[test]
    fn a_missing_deployment_dir_is_empty_not_an_error() {
        let missing = Path::new(".forest/deployment-does-not-exist-data-637");
        assert!(!missing.exists());

        assert_eq!(deployment_files_in(missing).unwrap(), Vec::<PathBuf>::new());
    }

    /// And a directory that does exist is still walked, files only.
    #[test]
    fn an_existing_dir_yields_its_files_and_no_directories() {
        let files = deployment_files_in(Path::new("src/cli/release")).unwrap();

        assert!(
            files.iter().any(|f| f.ends_with("annotate.rs")),
            "expected to find this very file, got {files:?}"
        );
        assert!(
            files.iter().all(|f| f.is_file()),
            "directories leaked into the walk: {files:?}"
        );
    }
}
