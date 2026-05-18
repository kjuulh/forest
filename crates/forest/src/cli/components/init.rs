use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::state::State;

#[derive(clap::Parser)]
pub struct InitCommand {
    /// Component name (e.g., "my-service")
    name: String,

    /// Organisation (e.g., "my-org")
    #[arg(long, default_value = "forest-contrib")]
    organisation: String,

    /// Implementation language
    #[arg(long, default_value = "rust")]
    language: String,

    /// Output directory (defaults to current directory)
    #[arg(long)]
    output: Option<PathBuf>,
}

impl InitCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let output_dir = self
            .output
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.name));

        if output_dir.exists() {
            anyhow::bail!("directory {} already exists", output_dir.display());
        }

        tokio::fs::create_dir_all(&output_dir)
            .await
            .context("create output directory")?;

        let name = &self.name;
        let org = &self.organisation;
        let pkg = name.replace('-', "_");

        // cue.mod/module.cue
        let cue_mod_dir = output_dir.join("cue.mod");
        tokio::fs::create_dir_all(&cue_mod_dir).await?;
        write_file(
            &cue_mod_dir.join("module.cue"),
            &format!(
                r#"module: "forest.sh/{org}/{name}@v0"
language: {{
	version: "v0.15.4"
}}
source: {{
	kind: "self"
}}
deps: {{
	"forest.sh/forest/sdk@v0": {{
		v: "v0.2.0"
	}}
}}
"#
            ),
        )
        .await?;

        // forest.component.cue
        write_file(
            &output_dir.join("forest.component.cue"),
            &format!(
                r#"package {pkg}

import "forest.sh/forest/sdk@v0"

// --- Input spec: what consuming projects must provide ---
#Spec: sdk.#ForestSpec & {{
	name:        string & =~"^[a-z][a-z0-9-]*$"
	environment: "dev" | "staging" | "prod"
}}

// --- Commands ---
#Commands: sdk.#ForestCommands & {{
	prepare: {{
		description: "Prepare deployment artifacts"
		input: {{}}
		output: {{}}
	}}
	status: {{
		description: "Check current status"
		input: {{}}
		output: {{
			healthy: bool
		}}
	}}
}}

// --- Lifecycle hooks ---
#Hooks: sdk.#ForestHooks & {{
	"forest/deployment": sdk.#ForestHook & {{
		prepare: {{
			description: "Prepare for deployment"
			input: {{}}
			output: {{}}
		}}
		release: {{
			description: "Perform deployment"
			input: {{
				release_id: string
			}}
			output: {{}}
		}}
		rollback: {{
			description: "Roll back deployment"
			input: {{
				release_id: string
			}}
		}}
	}}
}}
"#
            ),
        )
        .await?;

        // forest.cue
        write_file(
            &output_dir.join("forest.cue"),
            &format!(
                r#"package {pkg}

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {{
	name:         "{name}"
	organisation: "{org}"
}}

forest: component: sdk.#ForestComponent & {{
	name:    project.name
	version: "0.1.0"

	codegen: {{
		type:   "rust"
		output: "./crates/{name}/src/"
	}}

	upload: {{
		source: "./crates/{name}"
		type:   "rust"
		architectures: {{
			linux: {{
				amd64: {{}}
			}}
		}}
	}}
}}
"#
            ),
        )
        .await?;

        // Cargo.toml
        let crate_dir = output_dir.join("crates").join(name);
        tokio::fs::create_dir_all(crate_dir.join("src")).await?;

        // Detect if we're inside the forest workspace (dev mode) or standalone
        let forest_sdk_dep = if std::path::Path::new("../../crates/forest-sdk/Cargo.toml").exists()
        {
            r#"forest-sdk = { path = "../../../../crates/forest-sdk" }"#
        } else {
            r#"forest-sdk = { version = "0.1.0" }"#
        };

        write_file(
            &crate_dir.join("Cargo.toml"),
            &format!(
                r#"[package]
name = "{name}"
edition = "2024"
version = "0.1.0"

[dependencies]
{forest_sdk_dep}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#
            ),
        )
        .await?;

        // src/main.rs
        write_file(
            &crate_dir.join("src").join("main.rs"),
            r#"#[allow(dead_code)]
mod forestgen;

use forestgen::*;

struct Commands;
struct DeploymentHooks;

impl CommandHandler for Commands {
    fn prepare(
        &self,
        _spec: &Spec,
        _input: PrepareInput,
    ) -> Result<PrepareOutput, forest_sdk::Error> {
        Ok(PrepareOutput {})
    }

    fn status(
        &self,
        _spec: &Spec,
        _input: StatusInput,
    ) -> Result<StatusOutput, forest_sdk::Error> {
        Ok(StatusOutput { healthy: true })
    }
}

impl ForestDeploymentHookHandler for DeploymentHooks {
    fn prepare(
        &self,
        _spec: &Spec,
        _input: ForestDeploymentPrepareInput,
    ) -> Result<ForestDeploymentPrepareOutput, forest_sdk::Error> {
        Ok(ForestDeploymentPrepareOutput {})
    }

    fn release(
        &self,
        _spec: &Spec,
        _input: ForestDeploymentReleaseInput,
    ) -> Result<ForestDeploymentReleaseOutput, forest_sdk::Error> {
        Ok(ForestDeploymentReleaseOutput {})
    }

    fn rollback(
        &self,
        _spec: &Spec,
        _input: ForestDeploymentRollbackInput,
    ) -> Result<(), forest_sdk::Error> {
        Ok(())
    }
}

fn main() {
    let router = ComponentRouter::new(Commands, DeploymentHooks);
    forest_sdk::run_once(&router);
}
"#,
        )
        .await?;

        // Placeholder forestgen.rs (needs `forest components generate` to populate)
        write_file(
            &crate_dir.join("src").join("forestgen.rs"),
            "// Run `forest components generate --output .` to generate this file.\n",
        )
        .await?;

        tracing::info!("created component at {}", output_dir.display());
        eprintln!("Component '{name}' created at {}", output_dir.display());
        eprintln!();
        eprintln!("Next steps:");
        eprintln!("  cd {}", output_dir.display());
        eprintln!("  forest generate --output crates/{name}/src/");
        eprintln!("  forest build");
        eprintln!("  forest publish");

        Ok(())
    }
}

async fn write_file(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}
