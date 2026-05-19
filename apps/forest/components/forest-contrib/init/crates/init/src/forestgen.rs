// Hand-mirrored from forest.component.cue; replaced by codegen output once wired.
#![allow(clippy::upper_case_acronyms)]

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InitInput {
    pub project_name: String,
    pub organisation: String,
    #[serde(default = "default_template")]
    pub template: String,
    #[serde(default = "default_license")]
    pub license: String,
}

fn default_template() -> String {
    "rust-cli".to_string()
}
fn default_license() -> String {
    "MIT".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InitOutput {
    pub files_written: Vec<String>,
    pub template: String,
    pub work_dir: String,
}

pub trait CommandHandler: Send + Sync {
    /// Render a project skeleton into the workspace.
    fn init(
        &self,
        spec: &Spec,
        input: InitInput,
        context: &forest_sdk::CallContext,
    ) -> impl std::future::Future<Output = Result<InitOutput, forest_sdk::Error>> + Send;
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
            "commands/init" => {
                let input: InitInput = serde_json::from_value(input)?;
                let output = self.commands.init(spec, input, context).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/init".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Render a project skeleton (Cargo.toml, src/main.rs, README, .gitignore, \
                 forest.cue) into the workspace."
                    .to_string(),
            ),
        }]
    }
}
