// Hand-mirrored from forest.component.cue; replaced by codegen output once wired.
#![allow(clippy::upper_case_acronyms)]

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitCommitPushInput {
    pub repo: String,
    pub remote_url: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    pub message: String,
    #[serde(default = "default_user_name")]
    pub user_name: String,
    #[serde(default = "default_user_email")]
    pub user_email: String,
    #[serde(default)]
    pub allow_empty: bool,
}

fn default_branch() -> String {
    "main".to_string()
}
fn default_user_name() -> String {
    "forest-bot".to_string()
}
fn default_user_email() -> String {
    "forest-bot@local".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitCommitPushOutput {
    pub commit_sha: String,
    pub pushed_branch: String,
    pub remote_url: String,
}

pub trait CommandHandler: Send + Sync {
    /// Stage, commit, and push the contents of a working directory.
    fn git_commit_push(
        &self,
        spec: &Spec,
        input: GitCommitPushInput,
    ) -> impl std::future::Future<Output = Result<GitCommitPushOutput, forest_sdk::Error>> + Send;
}

pub struct ComponentRouter<C>
where
    C: CommandHandler + Send + Sync,
{
    commands: C,
}

impl<C> ComponentRouter<C>
where
    C: CommandHandler + Send + Sync,
{
    pub fn new(commands: C) -> Self {
        Self { commands }
    }
}

impl<C> forest_sdk::ComponentService<Spec> for ComponentRouter<C>
where
    C: CommandHandler + Send + Sync,
{
    async fn call(
        &self,
        method: &str,
        spec: &Spec,
        input: serde_json::Value,
        _context: &forest_sdk::CallContext,
    ) -> Result<serde_json::Value, forest_sdk::Error> {
        match method {
            "commands/git-commit-push" => {
                let input: GitCommitPushInput = serde_json::from_value(input)?;
                let output = self.commands.git_commit_push(spec, input).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/git-commit-push".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Stage, commit, and push the contents of a working directory \
                 to a remote URL."
                    .to_string(),
            ),
        }]
    }
}
