// Hand-mirrored from forest.component.cue; replaced by codegen output once wired.
#![allow(clippy::upper_case_acronyms)]

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitInitInput {
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default = "default_user_name")]
    pub user_name: String,
    #[serde(default = "default_user_email")]
    pub user_email: String,
    #[serde(default = "default_message")]
    pub message: String,
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
fn default_message() -> String {
    "initial commit".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitInitOutput {
    pub branch: String,
    pub initial_commit_sha: String,
    pub already_initialized: bool,
}

pub trait CommandHandler: Send + Sync {
    /// Initialise a fresh git repository at work_dir.
    fn git_init(
        &self,
        spec: &Spec,
        input: GitInitInput,
        context: &forest_sdk::CallContext,
    ) -> impl std::future::Future<Output = Result<GitInitOutput, forest_sdk::Error>> + Send;
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
        context: &forest_sdk::CallContext,
    ) -> Result<serde_json::Value, forest_sdk::Error> {
        match method {
            "commands/git-init" => {
                let input: GitInitInput = serde_json::from_value(input)?;
                let output = self.commands.git_init(spec, input, context).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/git-init".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Initialise a fresh git repository at work_dir with a configured author \
                 identity, a chosen branch name, and an empty initial commit."
                    .to_string(),
            ),
        }]
    }
}
