//! `forest-contrib/init@v0.1` — renders a small project skeleton into
//! work_dir based on `with:` inputs. v0 ships a single `rust-cli`
//! template inline; future versions can layer template-pack support
//! once we have a story for shipping templates separately.

#[allow(dead_code)]
mod forestgen;

use std::path::Path;

use forestgen::*;

struct Commands;

impl CommandHandler for Commands {
    async fn init(
        &self,
        _spec: &Spec,
        input: InitInput,
        context: &forest_sdk::CallContext,
    ) -> Result<InitOutput, forest_sdk::Error> {
        let work_dir = context
            .work_dir
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(".");

        let written = render_template(&input, work_dir)
            .map_err(|e| forest_sdk::Error::Handler(format!("render: {e:#}").into()))?;
        Ok(InitOutput {
            template: input.template,
            work_dir: work_dir.to_string(),
            files_written: written,
        })
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
                "//! {project} — scaffolded by forest-contrib/init@0.1.0\n\
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
                 Scaffolded by `forest-contrib/init@0.1.0`.\n"
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
    let router = ComponentRouter::new(Commands);
    forest_sdk::run_once(&router);
}
