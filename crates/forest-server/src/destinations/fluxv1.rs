use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::Context;
use forest_models::Destination;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem},
    temp_dir::TempDirectories,
};

/// Metadata files written to `.forest/` inside the releases directory.
/// Stored as YAML for readability. Kustomize does not recurse into
/// subdirectories, so `.forest/*.yaml` won't be applied by Flux.
struct ForestMetadataFiles {
    /// The deployment config from the prepare step.
    config_yaml: Option<String>,
    /// Annotation context: slug, source, context, reference, destination info.
    release_yaml: String,
    /// Original spec files as a YAML map {path: content}.
    spec_yaml: String,
}

/// Flux v2 GitOps destination.
///
/// Takes rendered deployment files from the artifact registry and pushes them
/// to a git repository following the multi-cluster Flux v2 directory layout:
///
/// ```text
/// clusters/<env>/<destination>/<cluster>/<namespace>/<project>/kustomization.yaml
/// releases/<env>/<destination>/<cluster>/<namespace>/<project>/<manifest files>
/// ```
///
/// The kustomization.yaml is a Flux Kustomization CR that points at the
/// corresponding releases path. Flux running on the target cluster reconciles
/// the manifests automatically.
///
/// Supports two modes via destination metadata:
/// - **Git mode** (`git_url`): clone → place files → commit → push
/// - **Local mode** (`local_path`): write files directly (for development)
pub struct FluxV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
    pub db: PgPool,
}

/// Parsed and validated metadata for a flux destination.
#[derive(Debug)]
struct FluxMetadata {
    cluster_name: String,
    namespace: String,
    git_url: Option<String>,
    git_branch: String,
    git_ssh_key_path: Option<String>,
    git_username: Option<String>,
    git_token: Option<String>,
    git_author_name: String,
    git_author_email: String,
    local_path: Option<PathBuf>,
    /// Optional webhook URL to trigger Flux reconciliation after push.
    /// Typically points at a Flux Receiver endpoint.
    reconcile_url: Option<String>,
}

impl FluxMetadata {
    fn from_metadata(metadata: &HashMap<String, String>) -> anyhow::Result<Self> {
        let cluster_name = metadata
            .get("cluster_name")
            .context("metadata 'cluster_name' is required for flux destinations")?
            .clone();

        let namespace = metadata
            .get("namespace")
            .context("metadata 'namespace' is required for flux destinations")?
            .clone();

        let git_url = metadata.get("git_url").cloned();
        let local_path = metadata.get("local_path").map(PathBuf::from);

        if git_url.is_none() && local_path.is_none() {
            anyhow::bail!(
                "flux destination requires either 'git_url' or 'local_path' in metadata"
            );
        }
        if git_url.is_some() && local_path.is_some() {
            anyhow::bail!(
                "flux destination cannot have both 'git_url' and 'local_path' in metadata"
            );
        }

        Ok(Self {
            cluster_name,
            namespace,
            git_url,
            git_branch: metadata
                .get("git_branch")
                .cloned()
                .unwrap_or_else(|| "main".to_string()),
            git_ssh_key_path: metadata.get("git_ssh_key_path").cloned(),
            git_username: metadata.get("git_username").cloned(),
            git_token: metadata.get("git_token").cloned(),
            git_author_name: metadata
                .get("git_author_name")
                .cloned()
                .unwrap_or_else(|| "forest-release".to_string()),
            git_author_email: metadata
                .get("git_author_email")
                .cloned()
                .unwrap_or_else(|| "forest@release.local".to_string()),
            local_path,
            reconcile_url: metadata.get("reconcile_url").cloned(),
        })
    }

    fn is_local(&self) -> bool {
        self.local_path.is_some()
    }

    /// Build the effective git URL with credentials embedded for HTTPS.
    fn effective_git_url(&self) -> anyhow::Result<String> {
        let url = self
            .git_url
            .as_ref()
            .context("git_url required for git mode")?;

        if let (Some(username), Some(token)) = (&self.git_username, &self.git_token) {
            if let Some(rest) = url.strip_prefix("https://") {
                return Ok(format!("https://{}:{}@{}", username, token, rest));
            }
        }

        Ok(url.clone())
    }

    /// Build environment variables for git SSH authentication.
    fn git_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        if let Some(ssh_key) = &self.git_ssh_key_path {
            env.insert(
                "GIT_SSH_COMMAND".to_string(),
                format!(
                    "ssh -i {} -o StrictHostKeyChecking=accept-new",
                    ssh_key
                ),
            );
        }
        env
    }

    /// Path within the gitops repo for release manifests.
    /// `releases/<env>/<destination>/<cluster_name>/<namespace>/<project>`
    fn releases_path(&self, env: &str, destination_name: &str, project: &str) -> PathBuf {
        PathBuf::from("releases")
            .join(env)
            .join(destination_name)
            .join(&self.cluster_name)
            .join(&self.namespace)
            .join(project)
    }

    /// Directory within the gitops repo for Flux Kustomization CRs.
    /// `clusters/<env>/<destination>/<cluster_name>/<namespace>`
    ///
    /// Individual project CRs are written as `<project>.yaml` inside this
    /// directory, alongside a plain kustomize `kustomization.yaml` that
    /// lists them as resources.
    fn clusters_dir(&self, env: &str, destination_name: &str) -> PathBuf {
        PathBuf::from("clusters")
            .join(env)
            .join(destination_name)
            .join(&self.cluster_name)
            .join(&self.namespace)
    }
}

enum Mode {
    Prepare,
    Apply,
}

impl FluxV1Destination {
    async fn run(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let flux_meta = FluxMetadata::from_metadata(&destination.metadata)
            .context("invalid flux destination metadata")?;

        // 1. Get artifact files
        let files = self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
            .context("get files for release")?;

        // 2. Write artifact files to temp dir
        let temp_dir = self.temp.create_emphemeral_temp().await?;
        for (path, content) in &files {
            let full_path = temp_dir.join(path);
            if let Some(parent) = full_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut file = tokio::fs::File::create_new(&full_path).await?;
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        }

        // 3. Match destination name against directory entries (same pattern as terraform)
        let env_dir = temp_dir.join(&destination.environment);
        let mut env_dir_entries = tokio::fs::read_dir(&env_dir)
            .await
            .context("read dir found no destinations for env")?;

        let mut matched = false;
        while let Some(entry) = env_dir_entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let entry_name = entry.file_name().to_string_lossy().to_string();

            let is_match = if let Ok(re) = regex::Regex::new(&entry_name) {
                re.is_match(&destination.name)
            } else {
                entry_name == destination.name
            };

            if !is_match {
                tracing::debug!(
                    "destination is not a match: files: {}, destination_name: {}",
                    entry_name,
                    destination.name
                );
                continue;
            }

            matched = true;

            // Build path to the rendered manifests
            let manifests_dir = env_dir
                .join(&entry_name)
                .join(&destination.destination_type.organisation)
                .join(format!(
                    "{}@{}",
                    destination.destination_type.name, destination.destination_type.version
                ));

            // 4. Collect manifest files (skips config.json — it goes to .forest/)
            let manifest_files = collect_manifest_files(&manifests_dir).await?;

            if manifest_files.is_empty() {
                anyhow::bail!(
                    "no manifest files found in: {}",
                    manifests_dir.display()
                );
            }

            // 5. Collect metadata files for .forest/ directory
            let config_yaml = read_config_as_yaml(&manifests_dir).await?;
            let release_yaml = self
                .build_release_yaml(release, destination, &flux_meta)
                .await?;
            let spec_yaml = self.build_spec_yaml(release).await?;

            let forest_metadata = ForestMetadataFiles {
                config_yaml,
                release_yaml,
                spec_yaml,
            };

            // 6. Resolve project name for directory structure
            let (organisation, project) = self.get_project_name(release).await?;
            let project_name = format!("{}-{}", organisation, project);

            // 7. Execute git or local mode
            if flux_meta.is_local() {
                self.run_local(
                    logger,
                    &flux_meta,
                    &manifest_files,
                    &forest_metadata,
                    &destination.environment,
                    &destination.name,
                    &project_name,
                    &mode,
                )
                .await?;
            } else {
                self.run_git(
                    logger,
                    &flux_meta,
                    &manifest_files,
                    &forest_metadata,
                    &destination.environment,
                    &destination.name,
                    &project_name,
                    &mode,
                )
                .await?;
            }
        }

        if !matched {
            anyhow::bail!("failed to find a destination match for submitted release");
        }

        Ok(())
    }

    async fn get_project_name(
        &self,
        release: &ReleaseItem,
    ) -> anyhow::Result<(String, String)> {
        let rec = sqlx::query!(
            "SELECT organisation, project FROM projects WHERE id = $1",
            release.project_id
        )
        .fetch_one(&self.db)
        .await
        .context("get project name for flux destination")?;

        Ok((rec.organisation, rec.project))
    }

    /// Build release.yaml content from the annotation context.
    async fn build_release_yaml(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
        meta: &FluxMetadata,
    ) -> anyhow::Result<String> {
        let rec = sqlx::query!(
            "SELECT slug, source, context, ref, created FROM annotations WHERE artifact_id = $1",
            release.artifact
        )
        .fetch_one(&self.db)
        .await
        .context("get annotation for release metadata")?;

        let release_info = serde_json::json!({
            "slug": rec.slug,
            "source": rec.source,
            "context": rec.context,
            "reference": rec.r#ref,
            "created_at": rec.created.to_rfc3339(),
            "destination": {
                "name": destination.name,
                "environment": destination.environment,
                "cluster": meta.cluster_name,
                "namespace": meta.namespace,
            }
        });

        serde_yaml::to_string(&release_info).context("serialize release metadata as yaml")
    }

    /// Build spec.yaml content from the artifact's spec files.
    async fn build_spec_yaml(&self, release: &ReleaseItem) -> anyhow::Result<String> {
        let spec_files = self
            .artifact_files
            .get_spec_files(&release.artifact)
            .await
            .context("get spec files")?;

        // Build YAML manually so multiline file contents use block scalar style (|)
        let mut out = String::new();
        for (path, content) in &spec_files {
            let key = path.to_string_lossy();
            if content.contains('\n') {
                out.push_str(&format!("{key}: |\n"));
                for line in content.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            } else {
                // Single-line value: use serde_yaml for proper escaping
                let val = serde_yaml::to_string(&content)
                    .context("serialize spec value")?
                    .trim_end()
                    .to_string();
                out.push_str(&format!("{key}: {val}\n"));
            }
        }

        Ok(out)
    }

    /// Generate a Flux Kustomization CR YAML.
    fn generate_kustomization_cr(
        namespace: &str,
        project: &str,
        releases_path: &Path,
    ) -> String {
        format!(
            r#"apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: {project}
  namespace: flux-system
spec:
  interval: 5m
  path: ./{releases_path}
  prune: true
  sourceRef:
    kind: GitRepository
    name: flux-system
  targetNamespace: {namespace}
  wait: true
  timeout: 3m
"#,
            project = project,
            namespace = namespace,
            releases_path = releases_path.display(),
        )
    }

    /// Write a plain kustomize `kustomization.yaml` in a releases directory,
    /// listing only the manifest files as resources (excludes `.forest/` metadata).
    async fn write_releases_kustomize_yaml(
        releases_dir: &Path,
        manifest_files: &[(String, String)],
    ) -> anyhow::Result<()> {
        let mut out = String::from("apiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nresources:\n");
        for (name, _) in manifest_files {
            out.push_str(&format!("  - {name}\n"));
        }
        tokio::fs::write(releases_dir.join("kustomization.yaml"), out.as_bytes()).await?;
        Ok(())
    }

    /// Write a plain kustomize `kustomization.yaml` in a clusters directory,
    /// listing all `*.yaml` files (except itself) as resources.
    async fn write_kustomize_yaml(clusters_dir: &Path) -> anyhow::Result<()> {
        let mut resources = Vec::new();
        let mut entries = tokio::fs::read_dir(clusters_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".yaml") && name.as_ref() != "kustomization.yaml" {
                resources.push(name.to_string());
            }
        }
        resources.sort();

        let mut out = String::from("apiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nresources:\n");
        for r in &resources {
            out.push_str(&format!("  - {r}\n"));
        }
        tokio::fs::write(clusters_dir.join("kustomization.yaml"), out.as_bytes()).await?;
        Ok(())
    }

    // ====== LOCAL MODE ======

    async fn run_local(
        &self,
        logger: &DestinationLogger,
        meta: &FluxMetadata,
        manifest_files: &[(String, String)],
        forest_metadata: &ForestMetadataFiles,
        env: &str,
        destination_name: &str,
        project: &str,
        mode: &Mode,
    ) -> anyhow::Result<()> {
        let local_root = meta.local_path.as_ref().context("local_path required")?;

        let releases_rel = meta.releases_path(env, destination_name, project);
        let clusters_dir_rel = meta.clusters_dir(env, destination_name);

        let releases_abs = local_root.join(&releases_rel);
        let clusters_dir_abs = local_root.join(&clusters_dir_rel);

        match mode {
            Mode::Prepare => {
                logger.log_stdout(&format!(
                    "[flux@1] local prepare: would write to {}",
                    releases_abs.display()
                ));

                for (name, _) in manifest_files {
                    logger.log_stdout(&format!("  {}", name));
                }
            }
            Mode::Apply => {
                logger.log_stdout(&format!(
                    "[flux@1] local apply: writing to {}",
                    releases_abs.display()
                ));

                // Clear and recreate releases directory
                if releases_abs.exists() {
                    tokio::fs::remove_dir_all(&releases_abs).await?;
                }
                tokio::fs::create_dir_all(&releases_abs).await?;
                write_manifest_files(&releases_abs, manifest_files).await?;
                write_forest_metadata(&releases_abs, forest_metadata).await?;
                Self::write_releases_kustomize_yaml(&releases_abs, manifest_files).await?;

                // Write Flux CR as <project>.yaml and regenerate kustomization.yaml
                tokio::fs::create_dir_all(&clusters_dir_abs).await?;
                let kustomization_cr =
                    Self::generate_kustomization_cr(&meta.namespace, project, &releases_rel);
                let cr_filename = format!("{project}.yaml");
                tokio::fs::write(
                    clusters_dir_abs.join(&cr_filename),
                    kustomization_cr.as_bytes(),
                )
                .await?;
                Self::write_kustomize_yaml(&clusters_dir_abs).await?;

                logger.log_stdout("[flux@1] local apply: done");
            }
        }

        Ok(())
    }

    // ====== GIT MODE ======

    async fn run_git(
        &self,
        logger: &DestinationLogger,
        meta: &FluxMetadata,
        manifest_files: &[(String, String)],
        forest_metadata: &ForestMetadataFiles,
        env: &str,
        destination_name: &str,
        project: &str,
        mode: &Mode,
    ) -> anyhow::Result<()> {
        let clone_dir = self.temp.create_emphemeral_temp().await?;
        let git_env = meta.git_env();
        let effective_url = meta.effective_git_url()?;

        // Step 1: Clone
        logger.log_stdout(&format!(
            "[flux@1] cloning gitops repo (branch: {})",
            meta.git_branch
        ));
        self.run_command(
            logger,
            &clone_dir,
            &[
                "clone",
                "--depth",
                "1",
                "--branch",
                &meta.git_branch,
                "--single-branch",
                &effective_url,
                "repo",
            ],
            &git_env,
        )
        .await
        .context("git clone")?;

        let repo_dir = clone_dir.join("repo");

        let releases_rel = meta.releases_path(env, destination_name, project);
        let clusters_dir_rel = meta.clusters_dir(env, destination_name);

        let releases_abs = repo_dir.join(&releases_rel);
        let clusters_dir_abs = repo_dir.join(&clusters_dir_rel);

        // Step 2: Clear and write release manifests + .forest/ metadata
        if releases_abs.exists() {
            tokio::fs::remove_dir_all(&releases_abs).await?;
        }
        tokio::fs::create_dir_all(&releases_abs).await?;
        write_manifest_files(&releases_abs, manifest_files).await?;
        write_forest_metadata(&releases_abs, forest_metadata).await?;
        Self::write_releases_kustomize_yaml(&releases_abs, manifest_files).await?;

        // Step 3: Write Flux CR as <project>.yaml and regenerate kustomization.yaml
        tokio::fs::create_dir_all(&clusters_dir_abs).await?;
        let kustomization_cr =
            Self::generate_kustomization_cr(&meta.namespace, project, &releases_rel);
        let cr_filename = format!("{project}.yaml");
        tokio::fs::write(
            clusters_dir_abs.join(&cr_filename),
            kustomization_cr.as_bytes(),
        )
        .await?;
        Self::write_kustomize_yaml(&clusters_dir_abs).await?;

        // Step 4: Stage changes
        self.run_command(logger, &repo_dir, &["add", "-A"], &git_env)
            .await
            .context("git add")?;

        // Step 5: Check if there are changes
        // git diff --cached --quiet returns non-zero when there are diffs
        let has_changes = self
            .run_command(logger, &repo_dir, &["diff", "--cached", "--quiet"], &git_env)
            .await
            .is_err();

        match mode {
            Mode::Prepare => {
                if has_changes {
                    logger.log_stdout("[flux@1] changes detected:");
                    // Show summary - ignore exit code (diff returns 1 when there are diffs)
                    let _ = self
                        .run_command(
                            logger,
                            &repo_dir,
                            &["diff", "--cached", "--stat"],
                            &git_env,
                        )
                        .await;
                } else {
                    logger.log_stdout("[flux@1] no changes detected");
                }
            }
            Mode::Apply => {
                if has_changes {
                    let commit_msg = format!(
                        "release: {}/{} to {}/{}",
                        env, project, destination_name, meta.cluster_name
                    );
                    self.run_command(
                        logger,
                        &repo_dir,
                        &[
                            "-c",
                            &format!("user.name={}", meta.git_author_name),
                            "-c",
                            &format!("user.email={}", meta.git_author_email),
                            "commit",
                            "-m",
                            &commit_msg,
                        ],
                        &git_env,
                    )
                    .await
                    .context("git commit")?;

                    logger.log_stdout("[flux@1] pushing to remote");
                    self.run_command(
                        logger,
                        &repo_dir,
                        &["push", "origin", &meta.git_branch],
                        &git_env,
                    )
                    .await
                    .context("git push")?;

                    logger.log_stdout("[flux@1] release pushed successfully");

                    // Trigger Flux reconciliation if a webhook URL is configured
                    if let Some(url) = &meta.reconcile_url {
                        Self::trigger_reconciliation(logger, url).await;
                    }
                } else {
                    logger.log_stdout("[flux@1] no changes to push, gitops repo is up to date");
                }
            }
        }

        Ok(())
    }

    // ====== RECONCILIATION ======

    async fn trigger_reconciliation(logger: &DestinationLogger, url: &str) {
        logger.log_stdout(&format!("[flux@1] triggering reconciliation via {url}"));
        match reqwest::Client::new()
            .post(url)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                logger.log_stdout("[flux@1] reconciliation triggered successfully");
            }
            Ok(resp) => {
                logger.log_stdout(&format!(
                    "[flux@1] reconciliation webhook returned {}",
                    resp.status()
                ));
            }
            Err(e) => {
                logger.log_stdout(&format!(
                    "[flux@1] reconciliation webhook failed: {e} (non-fatal)"
                ));
            }
        }
    }

    // ====== COMMAND EXECUTION ======

    async fn run_command(
        &self,
        logger: &DestinationLogger,
        cwd: &Path,
        args: &[&str],
        env: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let exe = std::env::var("GIT_EXE").unwrap_or_else(|_| "git".to_string());

        tracing::debug!(cwd =% cwd.display(), "running {} {}", exe, args.join(" "));

        let mut cmd = tokio::process::Command::new(&exe);
        cmd.current_dir(cwd)
            .envs(env)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut proc = cmd.spawn()?;

        if let Some(stdout) = proc.stdout.take() {
            let logger = logger.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("flux@1: {}", line);
                    logger.log_stdout(&line);
                }
            });
        }
        if let Some(stderr) = proc.stderr.take() {
            let logger = logger.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("flux@1: {}", line);
                    logger.log_stderr(&line);
                }
            });
        }

        let exit = proc.wait().await.context("command failed")?;
        if !exit.success() {
            anyhow::bail!(
                "{} {} failed: {}",
                exe,
                args.join(" "),
                exit.code().unwrap_or(-1)
            );
        }

        tracing::debug!("git command success");

        Ok(())
    }
}

#[async_trait::async_trait]
impl DestinationEdge for FluxV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "flux".into(),
            version: 1,
        }
    }

    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        FluxMetadata::from_metadata(metadata)?;
        Ok(())
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(logger, release, destination, Mode::Prepare)
            .await
            .context("flux prepare failed")
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(logger, release, destination, Mode::Apply)
            .await
            .context("flux release failed")
    }
}

// ====== HELPERS ======

/// Collect all files from a directory, returning (filename, content) pairs sorted by name.
/// Skips config.json which is handled separately via `.forest/config.json`.
async fn collect_manifest_files(dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("read manifest dir: {}", dir.display()))?;

    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip the forest config.json — it goes to .forest/config.json instead
            if name == "config.json" {
                continue;
            }
            let content = tokio::fs::read_to_string(entry.path()).await?;
            result.push((name, content));
        }
    }

    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// Read config.json from the manifest directory and convert to YAML.
/// The config is generated at `forest/config.json` inside the template directory.
async fn read_config_as_yaml(dir: &Path) -> anyhow::Result<Option<String>> {
    // config.json lives in the forest/ subdirectory
    let config_path = dir.join("forest").join("config.json");
    let content = if config_path.exists() {
        Some(tokio::fs::read_to_string(&config_path).await?)
    } else {
        // Fallback: check at the root level too
        let config_path = dir.join("config.json");
        if config_path.exists() {
            Some(tokio::fs::read_to_string(&config_path).await?)
        } else {
            None
        }
    };

    match content {
        Some(json_str) => {
            let value: serde_json::Value =
                serde_json::from_str(&json_str).context("parse config.json")?;
            let yaml = serde_yaml::to_string(&value).context("convert config to yaml")?;
            Ok(Some(yaml))
        }
        None => Ok(None),
    }
}

/// Write manifest files to a directory.
async fn write_manifest_files(dir: &Path, files: &[(String, String)]) -> anyhow::Result<()> {
    for (name, content) in files {
        let path = dir.join(name);
        let mut file = tokio::fs::File::create(&path).await?;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;
    }
    Ok(())
}

/// Write `.forest/` metadata directory inside the releases path.
/// Contains config.yaml, release.yaml, and spec.yaml. Kustomize does not
/// recurse into subdirectories, so these won't be applied by Flux.
async fn write_forest_metadata(
    releases_dir: &Path,
    metadata: &ForestMetadataFiles,
) -> anyhow::Result<()> {
    let forest_dir = releases_dir.join(".forest");
    tokio::fs::create_dir_all(&forest_dir).await?;

    // Write config.yaml (deployment config from prepare step)
    if let Some(config) = &metadata.config_yaml {
        tokio::fs::write(forest_dir.join("config.yaml"), config.as_bytes()).await?;
    }

    // Write release.yaml (annotation context + destination info)
    tokio::fs::write(
        forest_dir.join("release.yaml"),
        metadata.release_yaml.as_bytes(),
    )
    .await?;

    // Write spec.yaml (original spec files)
    tokio::fs::write(
        forest_dir.join("spec.yaml"),
        metadata.spec_yaml.as_bytes(),
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metadata(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn valid_git_metadata() -> HashMap<String, String> {
        make_metadata(&[
            ("cluster_name", "prod-eu-west-1"),
            ("namespace", "rust-podinfo"),
            ("git_url", "git@github.com:org/gitops.git"),
        ])
    }

    fn valid_local_metadata() -> HashMap<String, String> {
        make_metadata(&[
            ("cluster_name", "dev-local"),
            ("namespace", "rust-podinfo"),
            ("local_path", "/tmp/gitops"),
        ])
    }

    // ====== METADATA VALIDATION ======

    #[test]
    fn test_metadata_requires_cluster_name() {
        let meta = make_metadata(&[
            ("namespace", "ns"),
            ("git_url", "git@host:repo.git"),
        ]);
        let err = FluxMetadata::from_metadata(&meta).unwrap_err();
        assert!(err.to_string().contains("cluster_name"));
    }

    #[test]
    fn test_metadata_requires_namespace() {
        let meta = make_metadata(&[
            ("cluster_name", "prod"),
            ("git_url", "git@host:repo.git"),
        ]);
        let err = FluxMetadata::from_metadata(&meta).unwrap_err();
        assert!(err.to_string().contains("namespace"));
    }

    #[test]
    fn test_metadata_requires_git_url_or_local_path() {
        let meta = make_metadata(&[("cluster_name", "prod"), ("namespace", "ns")]);
        let err = FluxMetadata::from_metadata(&meta).unwrap_err();
        assert!(err.to_string().contains("git_url"));
    }

    #[test]
    fn test_metadata_rejects_both_git_and_local() {
        let meta = make_metadata(&[
            ("cluster_name", "prod"),
            ("namespace", "ns"),
            ("git_url", "git@host:repo.git"),
            ("local_path", "/tmp/local"),
        ]);
        let err = FluxMetadata::from_metadata(&meta).unwrap_err();
        assert!(err.to_string().contains("cannot have both"));
    }

    #[test]
    fn test_metadata_valid_git() {
        let meta = FluxMetadata::from_metadata(&valid_git_metadata()).unwrap();
        assert_eq!(meta.cluster_name, "prod-eu-west-1");
        assert_eq!(meta.namespace, "rust-podinfo");
        assert_eq!(meta.git_branch, "main");
        assert_eq!(meta.git_author_name, "forest-release");
        assert_eq!(meta.git_author_email, "forest@release.local");
        assert!(!meta.is_local());
    }

    #[test]
    fn test_metadata_valid_local() {
        let meta = FluxMetadata::from_metadata(&valid_local_metadata()).unwrap();
        assert!(meta.is_local());
        assert_eq!(meta.local_path.unwrap(), PathBuf::from("/tmp/gitops"));
    }

    #[test]
    fn test_metadata_custom_branch() {
        let mut m = valid_git_metadata();
        m.insert("git_branch".into(), "deploy".into());
        let meta = FluxMetadata::from_metadata(&m).unwrap();
        assert_eq!(meta.git_branch, "deploy");
    }

    #[test]
    fn test_metadata_custom_author() {
        let mut m = valid_git_metadata();
        m.insert("git_author_name".into(), "ci-bot".into());
        m.insert("git_author_email".into(), "ci@example.com".into());
        let meta = FluxMetadata::from_metadata(&m).unwrap();
        assert_eq!(meta.git_author_name, "ci-bot");
        assert_eq!(meta.git_author_email, "ci@example.com");
    }

    // ====== PATH GENERATION ======

    #[test]
    fn test_releases_path() {
        let meta = FluxMetadata::from_metadata(&valid_git_metadata()).unwrap();
        assert_eq!(
            meta.releases_path("dev", "k8s-dev-01", "rawpotion-rust-podinfo"),
            PathBuf::from(
                "releases/dev/k8s-dev-01/prod-eu-west-1/rust-podinfo/rawpotion-rust-podinfo"
            )
        );
    }

    #[test]
    fn test_clusters_dir() {
        let meta = FluxMetadata::from_metadata(&valid_git_metadata()).unwrap();
        assert_eq!(
            meta.clusters_dir("dev", "k8s-dev-01"),
            PathBuf::from(
                "clusters/dev/k8s-dev-01/prod-eu-west-1/rust-podinfo"
            )
        );
    }

    // ====== GIT URL CONSTRUCTION ======

    #[test]
    fn test_effective_git_url_ssh_passthrough() {
        let meta = FluxMetadata::from_metadata(&valid_git_metadata()).unwrap();
        assert_eq!(
            meta.effective_git_url().unwrap(),
            "git@github.com:org/gitops.git"
        );
    }

    #[test]
    fn test_effective_git_url_https_with_token() {
        let m = make_metadata(&[
            ("cluster_name", "prod"),
            ("namespace", "ns"),
            ("git_url", "https://github.com/org/repo.git"),
            ("git_username", "bot"),
            ("git_token", "ghp_abc123"),
        ]);
        let meta = FluxMetadata::from_metadata(&m).unwrap();
        assert_eq!(
            meta.effective_git_url().unwrap(),
            "https://bot:ghp_abc123@github.com/org/repo.git"
        );
    }

    #[test]
    fn test_effective_git_url_https_without_token() {
        let m = make_metadata(&[
            ("cluster_name", "prod"),
            ("namespace", "ns"),
            ("git_url", "https://github.com/org/repo.git"),
        ]);
        let meta = FluxMetadata::from_metadata(&m).unwrap();
        assert_eq!(
            meta.effective_git_url().unwrap(),
            "https://github.com/org/repo.git"
        );
    }

    // ====== GIT ENV ======

    #[test]
    fn test_git_env_with_ssh_key() {
        let mut m = valid_git_metadata();
        m.insert("git_ssh_key_path".into(), "/path/to/key".into());
        let meta = FluxMetadata::from_metadata(&m).unwrap();
        let env = meta.git_env();
        let ssh_cmd = env.get("GIT_SSH_COMMAND").unwrap();
        assert!(ssh_cmd.contains("/path/to/key"));
        assert!(ssh_cmd.contains("StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn test_git_env_without_ssh_key() {
        let meta = FluxMetadata::from_metadata(&valid_git_metadata()).unwrap();
        let env = meta.git_env();
        assert!(env.is_empty());
    }

    // ====== KUSTOMIZATION CR ======

    #[test]
    fn test_generate_kustomization_cr() {
        let cr = FluxV1Destination::generate_kustomization_cr(
            "rust-podinfo",
            "rawpotion-rust-podinfo",
            &PathBuf::from(
                "releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo",
            ),
        );
        assert!(cr.contains("apiVersion: kustomize.toolkit.fluxcd.io/v1"));
        assert!(cr.contains("kind: Kustomization"));
        assert!(cr.contains("name: rawpotion-rust-podinfo"));
        assert!(cr.contains("namespace: flux-system"));
        assert!(cr.contains("targetNamespace: rust-podinfo"));
        assert!(cr.contains(
            "path: ./releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo"
        ));
        assert!(cr.contains("prune: true"));
        assert!(cr.contains("kind: GitRepository"));
        assert!(cr.contains("name: flux-system"));
    }

    // ====== INTEGRATION: LOCAL MODE FILE PLACEMENT ======

    #[tokio::test]
    async fn test_local_mode_file_placement() {
        let local_root = tempfile::tempdir().unwrap();
        let meta = FluxMetadata {
            cluster_name: "dev-cluster-01".into(),
            namespace: "rust-podinfo".into(),
            git_url: None,
            git_branch: "main".into(),
            git_ssh_key_path: None,
            git_username: None,
            git_token: None,
            git_author_name: "test".into(),
            git_author_email: "test@test".into(),
            local_path: Some(local_root.path().to_path_buf()),
            reconcile_url: None,
        };

        let manifest_files = vec![
            (
                "10-namespace.yaml".to_string(),
                "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: rust-podinfo\n".to_string(),
            ),
            (
                "20-deployment.yaml".to_string(),
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: rust-podinfo\n"
                    .to_string(),
            ),
            (
                "30-service.yaml".to_string(),
                "apiVersion: v1\nkind: Service\nmetadata:\n  name: rust-podinfo\n".to_string(),
            ),
        ];

        let env = "dev";
        let destination_name = "k8s-dev-01";
        let project = "rawpotion-rust-podinfo";

        // Write releases
        let releases_abs = local_root
            .path()
            .join(meta.releases_path(env, destination_name, project));
        tokio::fs::create_dir_all(&releases_abs).await.unwrap();
        write_manifest_files(&releases_abs, &manifest_files)
            .await
            .unwrap();

        // Write kustomization CR as <project>.yaml + plain kustomization.yaml
        let clusters_dir_abs = local_root
            .path()
            .join(meta.clusters_dir(env, destination_name));
        tokio::fs::create_dir_all(&clusters_dir_abs).await.unwrap();
        let releases_rel = meta.releases_path(env, destination_name, project);
        let kustomization_cr =
            FluxV1Destination::generate_kustomization_cr(&meta.namespace, project, &releases_rel);
        let cr_filename = format!("{project}.yaml");
        tokio::fs::write(
            clusters_dir_abs.join(&cr_filename),
            kustomization_cr.as_bytes(),
        )
        .await
        .unwrap();
        FluxV1Destination::write_kustomize_yaml(&clusters_dir_abs)
            .await
            .unwrap();

        // Verify releases directory structure
        assert!(releases_abs.join("10-namespace.yaml").exists());
        assert!(releases_abs.join("20-deployment.yaml").exists());
        assert!(releases_abs.join("30-service.yaml").exists());

        let ns_content = tokio::fs::read_to_string(releases_abs.join("10-namespace.yaml"))
            .await
            .unwrap();
        assert!(ns_content.contains("kind: Namespace"));

        // Verify clusters directory structure — plain kustomize file
        let kust_path = clusters_dir_abs.join("kustomization.yaml");
        assert!(kust_path.exists());
        let kust_content = tokio::fs::read_to_string(&kust_path).await.unwrap();
        assert!(kust_content.contains("kind: Kustomization"));
        assert!(kust_content.contains(&cr_filename));

        // Verify Flux CR file
        let cr_path = clusters_dir_abs.join(&cr_filename);
        assert!(cr_path.exists());
        let cr_content = tokio::fs::read_to_string(&cr_path).await.unwrap();
        assert!(cr_content.contains("kustomize.toolkit.fluxcd.io"));
        assert!(cr_content.contains("targetNamespace: rust-podinfo"));
        assert!(cr_content.contains(&format!(
            "path: ./{}",
            releases_rel.display()
        )));

        // Verify full path structure
        let expected_releases = local_root.path().join(
            "releases/dev/k8s-dev-01/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo",
        );
        let expected_clusters_dir = local_root.path().join(
            "clusters/dev/k8s-dev-01/dev-cluster-01/rust-podinfo",
        );
        assert!(expected_releases.exists());
        assert!(expected_clusters_dir.exists());
        assert!(expected_clusters_dir.join("rawpotion-rust-podinfo.yaml").exists());
    }

    // ====== INTEGRATION: COLLECT MANIFEST FILES ======

    #[tokio::test]
    async fn test_collect_manifest_files() {
        let dir = tempfile::tempdir().unwrap();

        // Create some manifest files
        tokio::fs::write(dir.path().join("10-namespace.yaml"), "ns content")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("20-deployment.yaml"), "deploy content")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("30-service.yaml"), "svc content")
            .await
            .unwrap();
        // config.json should be skipped
        tokio::fs::write(dir.path().join("config.json"), "{}")
            .await
            .unwrap();

        let files = collect_manifest_files(dir.path()).await.unwrap();

        assert_eq!(files.len(), 3);
        assert_eq!(files[0].0, "10-namespace.yaml");
        assert_eq!(files[0].1, "ns content");
        assert_eq!(files[1].0, "20-deployment.yaml");
        assert_eq!(files[2].0, "30-service.yaml");
    }

    // ====== INTEGRATION: READ CONFIG AS YAML ======

    #[tokio::test]
    async fn test_read_config_as_yaml_in_forest_subdir() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("forest"))
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("forest").join("config.json"),
            r#"{"env":"dev"}"#,
        )
        .await
        .unwrap();

        let result = read_config_as_yaml(dir.path()).await.unwrap();
        let yaml = result.unwrap();
        assert!(yaml.contains("env: dev"));
    }

    #[tokio::test]
    async fn test_read_config_as_yaml_at_root() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("config.json"), r#"{"env":"dev"}"#)
            .await
            .unwrap();

        let result = read_config_as_yaml(dir.path()).await.unwrap();
        let yaml = result.unwrap();
        assert!(yaml.contains("env: dev"));
    }

    #[tokio::test]
    async fn test_read_config_as_yaml_absent() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_config_as_yaml(dir.path()).await.unwrap();
        assert_eq!(result, None);
    }

    // ====== INTEGRATION: WRITE FOREST METADATA ======

    #[tokio::test]
    async fn test_write_forest_metadata_full() {
        let dir = tempfile::tempdir().unwrap();

        let metadata = ForestMetadataFiles {
            config_yaml: Some("env: dev\nreplicas: 1\n".to_string()),
            release_yaml: "slug: test-slug\nsource: {}\n".to_string(),
            spec_yaml: "forest.cue: 'project: {}'\n".to_string(),
        };

        write_forest_metadata(dir.path(), &metadata).await.unwrap();

        let forest_dir = dir.path().join(".forest");
        assert!(forest_dir.exists());

        let config = tokio::fs::read_to_string(forest_dir.join("config.yaml"))
            .await
            .unwrap();
        assert!(config.contains("replicas"));

        let release = tokio::fs::read_to_string(forest_dir.join("release.yaml"))
            .await
            .unwrap();
        assert!(release.contains("test-slug"));

        let spec = tokio::fs::read_to_string(forest_dir.join("spec.yaml"))
            .await
            .unwrap();
        assert!(spec.contains("forest.cue"));
    }

    #[tokio::test]
    async fn test_write_forest_metadata_no_config() {
        let dir = tempfile::tempdir().unwrap();

        let metadata = ForestMetadataFiles {
            config_yaml: None,
            release_yaml: "slug: s\n".to_string(),
            spec_yaml: "{}\n".to_string(),
        };

        write_forest_metadata(dir.path(), &metadata).await.unwrap();

        let forest_dir = dir.path().join(".forest");
        assert!(forest_dir.exists());
        assert!(!forest_dir.join("config.yaml").exists());
        assert!(forest_dir.join("release.yaml").exists());
        assert!(forest_dir.join("spec.yaml").exists());
    }

    // ====== INTEGRATION: DIRECTORY CLEARING ======

    #[tokio::test]
    async fn test_directory_clearing_removes_stale_files() {
        let local_root = tempfile::tempdir().unwrap();

        let releases_dir = local_root
            .path()
            .join("releases/dev/dest/cluster/ns/project");
        let clusters_dir = local_root
            .path()
            .join("clusters/dev/dest/cluster/ns");

        // Pre-populate with stale files that should be removed
        tokio::fs::create_dir_all(&releases_dir).await.unwrap();
        tokio::fs::write(releases_dir.join("stale-old-manifest.yaml"), "old")
            .await
            .unwrap();
        tokio::fs::write(releases_dir.join("another-stale.yaml"), "old2")
            .await
            .unwrap();

        tokio::fs::create_dir_all(&clusters_dir).await.unwrap();
        tokio::fs::write(clusters_dir.join("old-project.yaml"), "old")
            .await
            .unwrap();

        // Simulate what run_git/run_local does: clear releases, update clusters
        tokio::fs::remove_dir_all(&releases_dir).await.unwrap();
        tokio::fs::create_dir_all(&releases_dir).await.unwrap();
        write_manifest_files(
            &releases_dir,
            &[("10-namespace.yaml".into(), "new content".into())],
        )
        .await
        .unwrap();

        // Clusters dir: write new CR + regenerate kustomization.yaml
        tokio::fs::write(clusters_dir.join("project.yaml"), "new cr")
            .await
            .unwrap();
        FluxV1Destination::write_kustomize_yaml(&clusters_dir)
            .await
            .unwrap();

        // Verify stale release files are gone
        assert!(!releases_dir.join("stale-old-manifest.yaml").exists());
        assert!(!releases_dir.join("another-stale.yaml").exists());

        // Verify new files are present
        assert!(releases_dir.join("10-namespace.yaml").exists());
        assert!(clusters_dir.join("project.yaml").exists());
        assert!(clusters_dir.join("kustomization.yaml").exists());
        // Old CR is still there (shared directory, not cleared)
        assert!(clusters_dir.join("old-project.yaml").exists());
    }

    // ====== INTEGRATION: GIT CLONE/COMMIT/PUSH CYCLE ======

    #[tokio::test]
    async fn test_git_mode_with_bare_repo() {
        // 1. Create a bare git repo
        let bare_dir = tempfile::tempdir().unwrap();
        let bare_path = bare_dir.path().join("gitops.git");

        let status = tokio::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(&bare_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git init --bare failed");

        // 2. Bootstrap: clone, add initial commit, push
        let bootstrap_dir = tempfile::tempdir().unwrap();
        let bootstrap_path = bootstrap_dir.path().join("repo");

        let status = tokio::process::Command::new("git")
            .args(["clone", &format!("file://{}", bare_path.display()), "repo"])
            .current_dir(bootstrap_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git clone (bootstrap) failed");

        // Create initial commit on main branch
        tokio::fs::write(bootstrap_path.join("README.md"), "# GitOps Repo\n")
            .await
            .unwrap();

        for (args, desc) in [
            (vec!["add", "-A"], "git add"),
            (
                vec![
                    "-c", "user.name=test",
                    "-c", "user.email=test@test",
                    "commit", "-m", "initial",
                ],
                "git commit",
            ),
            (vec!["push", "origin", "HEAD:main"], "git push"),
        ] {
            let status = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(&bootstrap_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap();
            assert!(status.success(), "{desc} failed");
        }

        // 3. Now simulate what FluxV1Destination.run_git does:
        // Clone, place files, commit, push
        let work_dir = tempfile::tempdir().unwrap();

        let status = tokio::process::Command::new("git")
            .args([
                "clone",
                "--depth", "1",
                "--branch", "main",
                "--single-branch",
                &format!("file://{}", bare_path.display()),
                "repo",
            ])
            .current_dir(work_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git clone (work) failed");

        let repo_dir = work_dir.path().join("repo");

        // Place release manifests
        let releases_dir = repo_dir.join("releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo");
        tokio::fs::create_dir_all(&releases_dir).await.unwrap();
        write_manifest_files(
            &releases_dir,
            &[
                ("10-namespace.yaml".into(), "apiVersion: v1\nkind: Namespace\n".into()),
                ("20-deployment.yaml".into(), "apiVersion: apps/v1\nkind: Deployment\n".into()),
            ],
        )
        .await
        .unwrap();

        // Place .forest/ metadata
        let forest_metadata = ForestMetadataFiles {
            config_yaml: Some("env: dev\ndestination: k8s-dev-01\n".to_string()),
            release_yaml: "slug: test-release\nsource: {}\ncontext:\n  title: Test\n".to_string(),
            spec_yaml: "forest.cue: 'project: {}'\n".to_string(),
        };
        write_forest_metadata(&releases_dir, &forest_metadata)
            .await
            .unwrap();

        // Place kustomization CR as <project>.yaml + plain kustomization.yaml
        let clusters_dir = repo_dir.join("clusters/dev/k8s-dev-01/prod-eu/rust-podinfo");
        tokio::fs::create_dir_all(&clusters_dir).await.unwrap();
        let cr = FluxV1Destination::generate_kustomization_cr(
            "rust-podinfo",
            "rawpotion-rust-podinfo",
            &PathBuf::from("releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo"),
        );
        tokio::fs::write(clusters_dir.join("rawpotion-rust-podinfo.yaml"), cr.as_bytes())
            .await
            .unwrap();
        FluxV1Destination::write_kustomize_yaml(&clusters_dir)
            .await
            .unwrap();

        // Stage and commit
        for (args, desc) in [
            (vec!["add", "-A"], "git add"),
            (
                vec![
                    "-c", "user.name=forest-release",
                    "-c", "user.email=forest@release.local",
                    "commit", "-m", "release: dev/rawpotion-rust-podinfo to k8s-dev-01/prod-eu",
                ],
                "git commit",
            ),
            (vec!["push", "origin", "main"], "git push"),
        ] {
            let status = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(&repo_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap();
            assert!(status.success(), "{desc} failed");
        }

        // 4. Verify: clone the bare repo fresh and check contents
        let verify_dir = tempfile::tempdir().unwrap();
        let status = tokio::process::Command::new("git")
            .args(["clone", &format!("file://{}", bare_path.display()), "verify"])
            .current_dir(verify_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git clone (verify) failed");

        let verify_repo = verify_dir.path().join("verify");

        // Check releases files exist
        let ns_file = verify_repo
            .join("releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo/10-namespace.yaml");
        assert!(ns_file.exists(), "namespace manifest not found in pushed repo");
        let ns_content = tokio::fs::read_to_string(&ns_file).await.unwrap();
        assert!(ns_content.contains("kind: Namespace"));

        let deploy_file = verify_repo
            .join("releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo/20-deployment.yaml");
        assert!(deploy_file.exists(), "deployment manifest not found");

        // Check clusters — plain kustomize file lists the Flux CR
        let kust_file = verify_repo
            .join("clusters/dev/k8s-dev-01/prod-eu/rust-podinfo/kustomization.yaml");
        assert!(kust_file.exists(), "kustomization.yaml not found");
        let kust_content = tokio::fs::read_to_string(&kust_file).await.unwrap();
        assert!(kust_content.contains("kustomize.config.k8s.io"));
        assert!(kust_content.contains("rawpotion-rust-podinfo.yaml"));

        // Check Flux CR file
        let cr_file = verify_repo
            .join("clusters/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo.yaml");
        assert!(cr_file.exists(), "Flux CR file not found");
        let cr_content = tokio::fs::read_to_string(&cr_file).await.unwrap();
        assert!(cr_content.contains("kustomize.toolkit.fluxcd.io"));
        assert!(cr_content.contains("targetNamespace: rust-podinfo"));
        assert!(cr_content.contains("path: ./releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo"));

        // Check .forest/ metadata was pushed
        let forest_dir = verify_repo
            .join("releases/dev/k8s-dev-01/prod-eu/rust-podinfo/rawpotion-rust-podinfo/.forest");
        assert!(forest_dir.exists(), ".forest/ directory not found");
        assert!(
            forest_dir.join("config.yaml").exists(),
            ".forest/config.yaml not found"
        );
        assert!(
            forest_dir.join("release.yaml").exists(),
            ".forest/release.yaml not found"
        );
        assert!(
            forest_dir.join("spec.yaml").exists(),
            ".forest/spec.yaml not found"
        );

        let release_yaml = tokio::fs::read_to_string(forest_dir.join("release.yaml"))
            .await
            .unwrap();
        assert!(release_yaml.contains("test-release"));

        // Check README still exists (not clobbered)
        assert!(verify_repo.join("README.md").exists());
    }
}
