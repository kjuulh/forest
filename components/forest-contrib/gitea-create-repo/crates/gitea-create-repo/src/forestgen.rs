// Hand-mirrored from forest.component.cue; replaced by codegen output once wired.
#![allow(clippy::upper_case_acronyms)]

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GiteaCreateRepoInput {
    pub base_url: String,
    #[serde(default)]
    pub org: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_private")]
    pub private: bool,
    #[serde(default)]
    pub auto_init: bool,
    #[serde(default = "default_branch")]
    pub default_branch: String,
    pub token_path: String,
}

fn default_private() -> bool {
    true
}
fn default_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GiteaCreateRepoOutput {
    pub id: i64,
    pub clone_url: String,
    pub ssh_url: String,
    pub html_url: String,
    pub full_name: String,
}

pub trait CommandHandler: Send + Sync {
    /// Create a repository on a Gitea instance via the REST API.
    fn gitea_create_repo(
        &self,
        spec: &Spec,
        input: GiteaCreateRepoInput,
    ) -> impl std::future::Future<Output = Result<GiteaCreateRepoOutput, forest_sdk::Error>> + Send;
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
            "commands/gitea-create-repo" => {
                let input: GiteaCreateRepoInput = serde_json::from_value(input)?;
                let output = self.commands.gitea_create_repo(spec, input).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/gitea-create-repo".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Create a repository on a Gitea instance via the REST API.".to_string(),
            ),
        }]
    }
}
