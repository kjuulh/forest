use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::state::State;

#[derive(clap::Parser)]
pub struct GenerateCommand {
    #[arg(long)]
    output: PathBuf,
}

impl GenerateCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.args([
            "def",
            "./forest.component.cue",
            "./spec.cue",
            "--out",
            "openapi",
        ]);

        let output = cmd.output().await?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;
        if !output.status.success() {
            anyhow::bail!("failed to spec from cue: {stdout}, {stderr}");
        }

        let codegen = forest_sdk_codegen::Codegen {
            options: forest_sdk_codegen::CodegenOptions {
                destination: self.output.display().to_string(),
                language: forest_sdk_codegen::CodegenLanguage::Rust,
            },
        };

        let generated_code = codegen.generate(stdout.trim())?;

        tokio::fs::create_dir_all(&self.output).await?;
        let mut file = tokio::fs::File::create(self.output.join("forestgen.rs")).await?;
        file.write_all(generated_code.as_bytes()).await?;
        file.flush().await?;

        Ok(())
    }
}
