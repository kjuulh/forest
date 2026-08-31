use std::path::Path;

use crate::state::State;

use anyhow::Context;

pub mod models;
use models::*;

const COMPONENT_SPEC_FILE_NAME: &str = "forest.component.toml";
const COMPONENT_CUE_FILE_NAME: &str = "forest.component.cue";

#[derive(Clone)]
pub struct ComponentParser {}

impl ComponentParser {
    pub async fn parse(&self, path: &Path) -> anyhow::Result<RawComponent> {
        // Try v1 (TOML) first
        if let Some(component_spec) = get_component_spec_path(path).await? {
            return Ok(RawComponent {
                component_spec,
                path: path.into(),
            });
        }

        // Try v2 (CUE) — extract minimal metadata from forest.cue via cue export
        if path.join(COMPONENT_CUE_FILE_NAME).exists() {
            if let Some(component_spec) = get_component_spec_from_cue(path).await? {
                return Ok(RawComponent {
                    component_spec,
                    path: path.into(),
                });
            }
        }

        anyhow::bail!("failed to find component in path")
    }
}

/// Parse a v2 component's metadata from forest.cue via `cue export`.
async fn get_component_spec_from_cue(path: &Path) -> anyhow::Result<Option<RawComponentSpec>> {
    let forest_cue = path.join("forest.cue");
    let spec_cue = path.join("spec.cue");

    if !forest_cue.exists() {
        return Ok(None);
    }

    // Run from inside the component, with relative file names. `cue` finds the
    // module root by walking up from its working directory, not from the files
    // it was handed, so evaluating an absolute path from elsewhere loads the
    // component with whatever module the caller happens to be standing in —
    // usually the project being built, which is not a CUE module at all. The
    // component's own cue.mod is then invisible and every import in it fails.
    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.current_dir(path);
        cmd.arg("export").arg("./forest.cue");
        if spec_cue.exists() {
            cmd.arg("./spec.cue");
        }
        cmd.arg("--out").arg("json");
        cmd
    })
    .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            "failed to parse v2 component CUE at {}: {}",
            path.display(),
            stderr
        );
        return Ok(None);
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    // Extract component metadata from forest.component section
    let component = doc.get("forest").and_then(|f| f.get("component"));

    let (name, organisation, version) = match component {
        Some(comp) => {
            let name = comp
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let version = comp
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0");
            // Organisation comes from project section
            let org = doc
                .get("project")
                .and_then(|p| p.get("organisation"))
                .and_then(|v| v.as_str())
                .unwrap_or("forest");
            (name.to_string(), org.to_string(), version.to_string())
        }
        None => {
            tracing::warn!(
                "v2 component at {} has no forest.component section",
                path.display()
            );
            return Ok(None);
        }
    };

    Ok(Some(RawComponentSpec {
        component: RawSpecComponent {
            name,
            organisation,
            version,
        },
        // v2 components don't use TOML fields — commands come from the binary
        dependencies: Default::default(),
        templates: Default::default(),
        init: Default::default(),
        requirements: Default::default(),
        commands: Default::default(),
    }))
}

async fn get_component_spec_path(path: &Path) -> Result<Option<RawComponentSpec>, anyhow::Error> {
    let mut dir_entries = tokio::fs::read_dir(path)
        .await
        .context(format!("component path does not exist: {}", path.display()))?;
    let mut spec_file = None;

    while let Some(entry) = dir_entries.next_entry().await? {
        if entry.file_name() == COMPONENT_SPEC_FILE_NAME {
            spec_file = Some(entry.path());
            break;
        }
    }

    let Some(spec_file) = spec_file else {
        return Ok(None);
    };

    let component_file_content = tokio::fs::read_to_string(spec_file).await?;

    let component_spec: RawComponentSpec = toml::from_str(&component_file_content)?;

    Ok(Some(component_spec))
}

pub trait ComponentParserState {
    fn component_parser(&self) -> ComponentParser;
}

impl ComponentParserState for State {
    fn component_parser(&self) -> ComponentParser {
        ComponentParser {}
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use std::path::Path;

    use crate::services::component_parser::{
        COMPONENT_CUE_FILE_NAME, ComponentParser, get_component_spec_from_cue,
        models::RawSpecComponent,
    };

    #[tokio::test]
    async fn can_parse_template() -> anyhow::Result<()> {
        let parser = ComponentParser {};

        let raw_component = parser
            .parse(&PathBuf::from("../../examples/rust-service-component/"))
            .await?;

        assert_eq!(
            raw_component.path,
            PathBuf::from("../../examples/rust-service-component/")
        );
        assert_eq!(
            raw_component.component_spec.component,
            RawSpecComponent {
                name: "rust-service".into(),
                organisation: "forest-contrib".into(),
                version: "0.1.0".into()
            }
        );

        let commands = &raw_component.component_spec.commands;
        assert!(commands.contains_key("build"), "missing 'build' command");
        assert!(
            commands.contains_key("validate"),
            "missing 'validate' command"
        );
        assert!(commands.contains_key("test"), "missing 'test' command");
        assert!(
            commands.contains_key("docker-build"),
            "missing 'docker-build' command"
        );
        assert!(commands.contains_key("status"), "missing 'status' command");
        assert_eq!(commands.len(), 5);

        Ok(())
    }

    /// A component that imports from its own module, laid out on disk exactly
    /// as a published one is: `cue.mod/module.cue`, a package to import, and a
    /// `forest.cue` that imports it.
    fn write_component_importing_its_own_module(root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join("cue.mod"))?;
        std::fs::create_dir_all(root.join("sub"))?;
        std::fs::write(
            root.join("cue.mod/module.cue"),
            "module: \"example.com/testmod@v0\"\nlanguage: version: \"v0.16.1\"\n",
        )?;
        std::fs::write(
            root.join("sub/sub.cue"),
            "package sub\n\nName: \"understory\"\n",
        )?;
        std::fs::write(
            root.join("forest.cue"),
            "package testmod\n\n\
             import \"example.com/testmod/sub\"\n\n\
             project: organisation: sub.Name\n\
             forest: component: {name: \"demo\", version: \"1.0.0\"}\n",
        )?;
        std::fs::write(root.join(COMPONENT_CUE_FILE_NAME), "package testmod\n")?;
        Ok(())
    }

    /// `cue` finds its module root by walking up from its *working directory*,
    /// not from the files it is handed. So a component was only parseable while
    /// forest happened to be standing inside it — which it never is: it parses
    /// components out of the cache while sitting in the project being built.
    ///
    /// The whole of a published component's `cue.mod` was invisible, and every
    /// import in it failed with "imports are unavailable because there is no
    /// cue.mod/module.cue file" — naming the very file that was sitting right
    /// there next to it.
    ///
    /// This test deliberately does not change the working directory: being
    /// parseable from somewhere else is the entire property.
    #[tokio::test]
    async fn a_component_is_parsed_against_its_own_module_not_the_callers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = dir.path().join("component");
        std::fs::create_dir_all(&component).expect("create component dir");
        write_component_importing_its_own_module(&component).expect("write component");

        let spec = get_component_spec_from_cue(&component)
            .await
            .expect("a component with a cue.mod parses from any working directory")
            .expect("forest.cue is present, so a spec is returned");

        // Resolved through the import, so this value only exists if the
        // component's own module was actually loaded.
        assert_eq!(
            spec.component.organisation, "understory",
            "organisation comes from the imported package, so reading it back \
             proves the import resolved"
        );
        assert_eq!(spec.component.name, "demo");
        assert_eq!(spec.component.version, "1.0.0");
    }

    /// `spec.cue` is passed alongside forest.cue when present, and it moved
    /// from an absolute path to a relative one at the same time as the working
    /// directory changed. If that pair ever disagrees, cue is handed a file
    /// that is not there.
    #[tokio::test]
    async fn a_components_spec_cue_is_still_picked_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = dir.path().join("component");
        std::fs::create_dir_all(&component).expect("create component dir");
        write_component_importing_its_own_module(&component).expect("write component");
        std::fs::write(
            component.join("spec.cue"),
            "package testmod\n\nspec: shape: \"documented\"\n",
        )
        .expect("write spec.cue");

        let spec = get_component_spec_from_cue(&component)
            .await
            .expect("forest.cue and spec.cue are unified")
            .expect("a spec is returned");

        // spec.cue is in the same package, so a conflict with forest.cue would
        // have failed the export outright and returned None.
        assert_eq!(spec.component.name, "demo");
        assert_eq!(spec.component.organisation, "understory");
    }

    /// Components without a cue.mod — everything published before the module
    /// file was included, and anything with no imports — must keep parsing.
    #[tokio::test]
    async fn a_component_with_no_module_still_parses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = dir.path().join("plain");
        std::fs::create_dir_all(&component).expect("create component dir");
        std::fs::write(
            component.join("forest.cue"),
            "project: organisation: \"understory\"\n\
             forest: component: {name: \"plain\", version: \"2.0.0\"}\n",
        )
        .expect("write forest.cue");
        std::fs::write(component.join(COMPONENT_CUE_FILE_NAME), "").expect("write marker");

        let spec = get_component_spec_from_cue(&component)
            .await
            .expect("a component without a module parses")
            .expect("a spec is returned");

        assert_eq!(spec.component.name, "plain");
        assert_eq!(spec.component.version, "2.0.0");
    }

    /// A component whose CUE does not evaluate is skipped rather than fatal:
    /// `parse` turns the None into "failed to find component in path", which
    /// the caller now warns about instead of taking the whole scan down.
    #[tokio::test]
    async fn a_component_that_does_not_evaluate_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = dir.path().join("broken");
        std::fs::create_dir_all(&component).expect("create component dir");
        std::fs::write(
            component.join("forest.cue"),
            "import \"example.com/nope/missing\"\n\nproject: organisation: missing.X\n",
        )
        .expect("write forest.cue");
        std::fs::write(component.join(COMPONENT_CUE_FILE_NAME), "").expect("write marker");

        let spec = get_component_spec_from_cue(&component)
            .await
            .expect("a component that fails to evaluate is not an error here");

        assert!(
            spec.is_none(),
            "an unevaluatable component yields no spec rather than propagating"
        );
    }

    /// The end-to-end shape: `parse` dispatches to the CUE path via the
    /// forest.component.cue marker, so the fix has to hold through it and not
    /// just in the helper.
    #[tokio::test]
    async fn parse_reaches_the_cue_path_and_resolves_the_module() {
        let dir = tempfile::tempdir().expect("tempdir");
        let component = dir.path().join("component");
        std::fs::create_dir_all(&component).expect("create component dir");
        write_component_importing_its_own_module(&component).expect("write component");

        let raw = ComponentParser {}
            .parse(&component)
            .await
            .expect("parse resolves a v2 component from any working directory");

        assert_eq!(
            raw.component_spec.component,
            RawSpecComponent {
                name: "demo".into(),
                organisation: "understory".into(),
                version: "1.0.0".into(),
            }
        );
    }
}
