// Hand-mirrored from forest.component.cue. Will be replaced by output of
// `forest components build` once we wire codegen into the toolchain;
// kept manually in sync until then.
#![allow(clippy::upper_case_acronyms)]

use std::collections::HashMap;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Spec {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderTemplateInput {
    pub src: String,
    pub dest: String,
    #[serde(default)]
    pub vars: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderTemplateOutput {
    pub files_rendered: i64,
    pub src: String,
    pub dest: String,
}

pub trait CommandHandler: Send + Sync {
    /// Walk a source directory, interpolate `{{var}}` placeholders in
    /// file contents and path components, write the rendered tree to a
    /// destination directory.
    fn render_template(
        &self,
        spec: &Spec,
        input: RenderTemplateInput,
    ) -> impl std::future::Future<Output = Result<RenderTemplateOutput, forest_sdk::Error>> + Send;
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
            "commands/render-template" => {
                let input: RenderTemplateInput = serde_json::from_value(input)?;
                let output = self.commands.render_template(spec, input).await?;
                serde_json::to_value(output).map_err(forest_sdk::Error::Deserialization)
            }
            other => Err(forest_sdk::Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<forest_sdk::MethodDescriptor> {
        vec![forest_sdk::MethodDescriptor {
            name: "commands/render-template".to_string(),
            kind: forest_sdk::MethodKind::Command,
            description: Some(
                "Walk a source directory, interpolate {{var}} placeholders in file \
                 contents and path components, write the rendered tree to a \
                 destination directory."
                    .to_string(),
            ),
        }]
    }
}
