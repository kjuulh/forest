//! `forest:init@v1` — full-directory project scaffold.
//!
//! Speaks the Forest components-v2 SDK protocol (v1.1):
//!   `./forest-component-init <method> '<payload>'`
//! with `_meta/describe` for capability discovery and `commands/init`
//! to actually render.
//!
//! Inputs (the `input` field of the payload):
//!   project_name: string   (required)
//!   organisation: string   (required)
//!   template:     string   ("rust-cli" — only template supported in v0)
//!   license:      string   ("MIT" by default)
//!
//! Output: `{ files_written: [string] }`. Files land at
//! `context.work_dir/<path>` so the runner controls where the
//! scaffold goes (typically `/work` inside an exec VM, but the same
//! binary is reusable from `forest run` on a dev box).
//!
//! This is intentionally tiny — one template, no external deps beyond
//! the SDK. The shape is what matters; richer template registries layer
//! on top once the protocol path is proven.

use std::path::Path;

use forest_sdk::{
    CallContext, ComponentService, Error, MethodDescriptor, MethodKind, run_once,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
struct InitSpec {}

#[derive(Debug, Deserialize)]
struct InitInput {
    project_name: String,
    organisation: String,
    #[serde(default = "default_template")]
    template: String,
    #[serde(default = "default_license")]
    license: String,
}

fn default_template() -> String {
    "rust-cli".to_string()
}

fn default_license() -> String {
    "MIT".to_string()
}

#[derive(Debug, Serialize)]
struct InitOutput {
    files_written: Vec<String>,
    template: String,
    work_dir: String,
}

struct InitComponent;

impl ComponentService<InitSpec> for InitComponent {
    async fn call(
        &self,
        method: &str,
        _spec: &InitSpec,
        input: serde_json::Value,
        context: &CallContext,
    ) -> Result<serde_json::Value, Error> {
        match method {
            "commands/init" => {
                let input: InitInput = serde_json::from_value(input)?;
                let work_dir = context
                    .work_dir
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(".");
                let written = render_template(&input, work_dir).map_err(|e| {
                    Error::Handler(format!("render failed: {e:#}").into())
                })?;
                let out = InitOutput {
                    template: input.template,
                    work_dir: work_dir.to_string(),
                    files_written: written,
                };
                Ok(serde_json::to_value(out)?)
            }
            other => Err(Error::MethodNotFound(other.to_string())),
        }
    }

    fn methods(&self) -> Vec<MethodDescriptor> {
        vec![MethodDescriptor {
            name: "commands/init".to_string(),
            kind: MethodKind::Command,
            description: Some(
                "Render a project skeleton (Cargo.toml, src/main.rs, README, .gitignore) \
                 into work_dir."
                    .to_string(),
            ),
        }]
    }
}

fn render_template(input: &InitInput, work_dir: &str) -> anyhow::Result<Vec<String>> {
    let files: Vec<(&str, String)> = match input.template.as_str() {
        "rust-cli" => rust_cli_template(input),
        other => anyhow::bail!(
            "unknown template: {other:?} (only 'rust-cli' is supported in v0)"
        ),
    };

    let mut written = Vec::with_capacity(files.len());
    for (rel_path, content) in files {
        let full = Path::new(work_dir).join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("create_dir_all {}: {e}", parent.display()))?;
        }
        std::fs::write(&full, content)
            .map_err(|e| anyhow::anyhow!("write {}: {e}", full.display()))?;
        written.push(rel_path.to_string());
    }
    Ok(written)
}

fn rust_cli_template(input: &InitInput) -> Vec<(&'static str, String)> {
    let project = &input.project_name;
    let org = &input.organisation;
    let license = &input.license;
    vec![
        (
            "Cargo.toml",
            format!(
                "[package]\n\
                 name = \"{project}\"\n\
                 version = \"0.1.0\"\n\
                 edition = \"2024\"\n\
                 license = \"{license}\"\n\
                 # Owned by: {org}\n\
                 \n\
                 [dependencies]\n"
            ),
        ),
        (
            "src/main.rs",
            format!(
                "//! {project} — scaffolded by forest:init@v1\n\
                 \n\
                 fn main() {{\n    \
                     println!(\"Hello from {project} ({org})!\");\n\
                 }}\n"
            ),
        ),
        (
            "README.md",
            format!(
                "# {project}\n\
                 \n\
                 - Owner: **{org}**\n\
                 - License: **{license}**\n\
                 \n\
                 Scaffolded by `forest:init@v1`.\n"
            ),
        ),
        (".gitignore", "/target\n*.swp\n.DS_Store\n".to_string()),
        (
            "forest.cue",
            format!(
                "package forest\n\
                 \n\
                 project: {{\n    \
                     name:         \"{project}\"\n    \
                     organisation: \"{org}\"\n\
                 }}\n"
            ),
        ),
    ]
}

fn main() {
    run_once(&InitComponent);
}
