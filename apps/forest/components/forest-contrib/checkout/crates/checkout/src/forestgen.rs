// Hand-mirrored from forest.component.cue; replaced by codegen output once wired.
#![allow(clippy::upper_case_acronyms)]

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckoutInput {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: i64,
    pub dest: String,
}

fn default_depth() -> i64 {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckoutOutput {
    pub commit_sha: String,
    pub branch: String,
    pub dest: String,
}

pub trait CommandHandler: Send + Sync {
    /// Shallow-clone a git repository into a destination directory.
    fn checkout(
        &self,
        spec: &Spec,
        input: CheckoutInput,
    ) -> impl std::future::Future<Output = Result<CheckoutOutput, forest_sdk::Error>> + Send;
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
            "commands/checkout" => {
                let input: CheckoutInput = serde_json::from_value(input)?;
                let output = self.commands.checkout(spec, input).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/checkout".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Shallow-clone a git repository into a destination directory.".to_string(),
            ),
        }]
    }
}
