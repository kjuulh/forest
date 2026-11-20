use crate::cli::components::publish::PublishCommand;
use crate::state::State;

const COMPONENT_ORDER: &[&str] = &[
    "components/forest/sdk",
    "components/forest/deployment",
    "components/forest-contrib/build-rust",
    "components/forest-contrib/build-go",
    "components/forest-contrib/build-docker",
];

#[derive(clap::Parser)]
pub struct BootstrapCommand {
    /// Keep publishing the remaining components even if one fails, then report
    /// the failures at the end. Without this, the first failure aborts.
    #[arg(long)]
    keep_going: bool,

    /// Publish only components whose path contains this substring (e.g.
    /// `build-` to publish just the build components).
    #[arg(long)]
    filter: Option<String>,
}

impl BootstrapCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let start = std::env::current_dir()?;
        let mut published = 0usize;
        let mut failures: Vec<String> = Vec::new();

        for rel in COMPONENT_ORDER {
            if let Some(filter) = &self.filter {
                if !rel.contains(filter.as_str()) {
                    continue;
                }
            }

            let dir = start.join(rel);
            if !dir.join("forest.cue").exists() {
                tracing::warn!("skipping {rel}: no forest.cue (not found?)");
                continue;
            }

            tracing::info!("publishing {rel}");
            std::env::set_current_dir(&dir)?;
            let result = PublishCommand::for_bootstrap().execute(state).await;
            std::env::set_current_dir(&start)?;

            match result {
                Ok(()) => {
                    println!("✓ published {rel}");
                    published += 1;
                }
                Err(e) => {
                    eprintln!("✗ {rel}: {e:#}");
                    if !self.keep_going {
                        return Err(e.context(format!("bootstrap failed publishing {rel}")));
                    }
                    failures.push((*rel).to_string());
                }
            }
        }

        println!("bootstrap: published {published} component(s)");
        if !failures.is_empty() {
            anyhow::bail!(
                "bootstrap: {} component(s) failed: {}",
                failures.len(),
                failures.join(", ")
            );
        }
        Ok(())
    }
}
