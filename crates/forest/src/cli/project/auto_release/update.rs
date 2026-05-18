use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Policy name to update
    #[arg(long)]
    name: Option<String>,

    /// Enable or disable the policy
    #[arg(long)]
    enabled: Option<bool>,

    /// Regex to match against the branch name
    #[arg(long)]
    branch: Option<String>,

    /// Regex to match against the annotation title
    #[arg(long)]
    title: Option<String>,

    /// Regex to match against the source author
    #[arg(long)]
    author: Option<String>,

    /// Regex to match against the commit message
    #[arg(long)]
    commit_message: Option<String>,

    /// Regex to match against the source type
    #[arg(long)]
    source_type: Option<String>,

    /// Target environments to release to (replaces existing; can be repeated)
    #[arg(long = "env", short = 'e')]
    target_environments: Vec<String>,

    /// Target destinations to release to (replaces existing; can be repeated)
    #[arg(long = "dest", short = 'd')]
    target_destinations: Vec<String>,

    /// Whether to force-release (cancel queued releases)
    #[arg(long)]
    force: Option<bool>,

    /// Trigger the project's release pipeline instead of deploying directly
    #[arg(long)]
    use_pipeline: Option<bool>,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => prompts::select_project(state, &organisation).await?,
        };

        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Policy name:").prompt()?,
        };

        let policy = state
            .grpc_client()
            .update_auto_release_policy(
                &organisation,
                &project,
                &name,
                self.enabled,
                self.branch.clone(),
                self.title.clone(),
                self.author.clone(),
                self.commit_message.clone(),
                self.source_type.clone(),
                self.target_environments.clone(),
                self.target_destinations.clone(),
                self.force,
                self.use_pipeline,
            )
            .await
            .context("update auto-release policy")?;

        let status = if policy.enabled { "enabled" } else { "disabled" };
        eprintln!("Updated auto-release policy '{}' ({})", policy.name, status);

        Ok(())
    }
}
