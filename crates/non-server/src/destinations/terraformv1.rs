use std::{path::Path, process::Stdio};

use anyhow::Context;
use non_models::Destination;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{
    destinations::{DestinationEdge, DestinationIndex},
    services::{artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem},
    temp_dir::TempDirectories,
};

pub struct TerraformV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
}

impl TerraformV1Destination {
    async fn run(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let temp_dir = self.temp.create_emphemeral_temp().await?;
        let files = self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
            .context("get files for release")?;

        // 1. Fill temp dir with the correct files
        for (path, content) in files {
            let path = temp_dir.join(path);
            tracing::debug!("placing files in: {}", path.display());

            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("terraform create dir")?;
            }

            let mut file = tokio::fs::File::create_new(path)
                .await
                .context("terraform create file")?;
            file.write_all(content.as_bytes())
                .await
                .context("terraform write content")?;
            file.flush().await.context("terraform flush file")?
        }

        let env_dir = &temp_dir.join(&destination.environment);

        let mut env_dir_entries = tokio::fs::read_dir(env_dir)
            .await
            .context("read dir found no destinations for env")?;

        let mut matched = false;
        while let Some(env_dir_entry) = env_dir_entries.next_entry().await? {
            let entry = env_dir_entry.file_type().await?;
            if !entry.is_dir() {
                // Ignore non dirs
                continue;
            }

            let entry_name = env_dir_entry.file_name();
            let entry_name = entry_name.to_string_lossy().to_string();
            if let Ok(re) = regex::Regex::new(&entry_name.clone()) {
                if !re.is_match(&destination.name) {
                    tracing::debug!(
                        "destination (regex) is not a match: files: {}, destination_name: {}",
                        entry_name,
                        destination.name
                    );
                    continue;
                }
            } else if entry_name != destination.name {
                tracing::debug!(
                    "destination is not a match: files: {}, destination_name: {}",
                    entry_name,
                    destination.name
                );
                continue;
            }

            matched = true;

            let dir = env_dir
                .join(entry_name) // find name that matches the dir
                .join(&destination.destination_type.organisation)
                .join(format!(
                    "{}@{}",
                    destination.destination_type.name, destination.destination_type.version
                ));

            // 2. Run terraform command over it
            self.run_command(destination, &dir, &["init"])
                .await
                .context("terraform init")?;

            match mode {
                Mode::Prepare => {
                    tracing::info!("running terraform plan");
                    self.run_command(destination, &dir, &["plan"])
                        .await
                        .context("terraform plan")?;
                }
                Mode::Apply => {
                    tracing::info!("running terraform apply");
                    self.run_command(destination, &dir, &["apply", "-auto-approve"])
                        .await
                        .context("terraform apply")?;
                }
            }
        }

        if !matched {
            anyhow::bail!("failed to find a destination match for submitted release");
        }

        Ok(())
    }

    async fn run_command(
        &self,
        destination: &Destination,
        path: &Path,
        args: &[&str],
    ) -> anyhow::Result<()> {
        tracing::debug!(path =% path.display(), "running terraform {}", args.join(" "));

        let exe = std::env::var("TERRAFORM_EXE").unwrap_or("terraform".to_string());

        let mut cmd = tokio::process::Command::new(exe);
        cmd.current_dir(path)
            .env("NO_COLOR", "1")
            .env("TF_IN_AUTOMATION", "true")
            .env("CI", "true");

        for (k, v) in &destination.metadata {
            cmd.env(format!("TF_VAR_{}", k), v);
        }

        let mut proc = cmd
            .args(args)
            .arg("-no-color")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        if let Some(stdout) = proc.stdout.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("terraform@1: {}", line);
                }
            });
        }
        if let Some(stderr) = proc.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("terraform@1: {}", line);
                }
            });
        }

        let exit = proc.wait().await.context("terraform failed")?;
        if !exit.success() {
            anyhow::bail!("terraform failed: {}", exit.code().unwrap_or(-1));
        }

        tracing::debug!("terraform command success");

        Ok(())
    }
}

#[async_trait::async_trait]
impl DestinationEdge for TerraformV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "non".into(),
            name: "terraform".into(),
            version: 1,
        }
    }
    async fn prepare(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(release, destination, Mode::Prepare)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }

    async fn release(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(release, destination, Mode::Apply)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }
}

enum Mode {
    Prepare,
    Apply,
}
