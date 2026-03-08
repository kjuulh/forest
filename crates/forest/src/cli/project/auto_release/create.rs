use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Policy name (unique per project)
    #[arg(long)]
    name: Option<String>,

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

    /// Regex to match against the source type (e.g. "github_actions", "ci")
    #[arg(long)]
    source_type: Option<String>,

    /// Target environments to release to (can be repeated)
    #[arg(long = "env", short = 'e')]
    target_environments: Vec<String>,

    /// Target destinations to release to (can be repeated)
    #[arg(long = "dest", short = 'd')]
    target_destinations: Vec<String>,

    /// Whether to force-release (cancel queued releases)
    #[arg(long, default_value_t = false)]
    force: bool,

    /// Trigger the project's release pipeline instead of deploying directly
    #[arg(long, default_value_t = false)]
    use_pipeline: bool,
}

impl CreateCommand {
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
            .create_auto_release_policy(
                &organisation,
                &project,
                &name,
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
            .context("create auto-release policy")?;

        println!("Created auto-release policy '{}'", policy.name);

        if let Some(bp) = &policy.branch_pattern {
            println!("  branch:         {bp}");
        }
        if let Some(tp) = &policy.title_pattern {
            println!("  title:          {tp}");
        }
        if let Some(ap) = &policy.author_pattern {
            println!("  author:         {ap}");
        }
        if !policy.target_environments.is_empty() {
            println!("  environments:   {}", policy.target_environments.join(", "));
        }
        if !policy.target_destinations.is_empty() {
            println!("  destinations:   {}", policy.target_destinations.join(", "));
        }
        if policy.force_release {
            println!("  force:          true");
        }

        Ok(())
    }
}
