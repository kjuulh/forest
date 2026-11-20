use crate::cli::components::publish::{
    cue_component_version, effective_version, export_forest_cue,
};
use crate::state::State;

#[derive(clap::Parser)]
pub struct VersionCommand {
    /// Override the version exactly as `forest publish --version` would.
    #[arg(
        long = "version",
        value_name = "VERSION",
        env = "FOREST_COMPONENT_VERSION"
    )]
    version: Option<String>,
}

impl VersionCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let doc = export_forest_cue(&std::env::current_dir()?).await?;
        println!(
            "{}",
            effective_version(self.version.as_deref(), cue_component_version(&doc))
        );
        Ok(())
    }
}
