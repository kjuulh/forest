//! Hidden dev command: publish all in-repo workspace components, in dependency
//! order, to the configured registry/server (DATA-312).
//!
//! Bringing tests up against a fresh local server otherwise means republishing
//! every component by hand. This iterates a fixed, ordered list and runs the
//! normal `forest components publish` flow in each directory.
//!
//! The order matters: the SDK module (`forest/sdk`) must land first because
//! every other component imports it, then the deployment contract, then the
//! leaf components. It's a hardcoded list for now — add entries as components
//! are added.
//!
//! Binary components (Rust/Go/Docker) must already be built; this command only
//! publishes. Run `cargo build` (or `forest run build` per component) first.

use crate::cli::components::publish::PublishCommand;
use crate::state::State;

/// Component directories to publish, relative to the current working directory
/// (run from the `apps/forest` workspace root), in dependency order.
const COMPONENT_ORDER: &[&str] = &[
    // Foundation — everything imports the SDK; the deployment contract next.
    "components/forest/sdk",
    "components/forest/deployment",
    // Build components (DATA-312).
    "components/forest-contrib/build-rust",
    "components/forest-contrib/build-go",
    "components/forest-contrib/build-docker",
    // Other v2 contrib components.
    // "components/forest-contrib/init",
    // "components/forest-contrib/git-init",
    // "components/forest-contrib/checkout",
    // "components/forest-contrib/git-commit-push",
    // "components/forest-contrib/gitea-create-repo",
    // "components/forest-contrib/render-template",
    // "components/forest-contrib/terraform-service",
    // "components/forest-contrib/ecs-service",
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
            // Switch cwd so the publish flow (which keys off the current
            // directory) targets this component. Restored after each so a
            // failure can't strand us in a child directory.
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
