use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    component_cache::models::{CacheComponent, CacheComponentCommand, CacheComponentSource},
    models::{CommandName, CommandSource, DependencyType, Project, ProjectValue},
    services::{
        component_binary,
        component_deno,
        components::{ComponentsService, ComponentsServiceState},
        temp_directories::{TempDirectories, TempDirectoriesState},
    },
    state::State,
};

use anyhow::Context;
pub mod models;
pub use models::*;

const FOREST_PROJECT_TOML_FILE: &str = "forest.toml";
const FOREST_PROJECT_NICKEL_FILE: &str = "forest.ncl";
const FOREST_PROJECT_YAML_FILE: &str = "forest.yaml";
const FOREST_PROJECT_CUE_FILE: &str = "forest.cue";

pub struct ProjectParser {
    component_service: ComponentsService,
    temp: TempDirectories,
}

impl ProjectParser {
    pub async fn get_project(&self) -> anyhow::Result<Project> {
        tracing::trace!("getting project");
        let mut project = self.parse_project_file().await?;

        let components = self.get_project_components(&project).await?;

        for component in components {
            // Check if this is a v2 component (has forest.component.cue)
            if let CacheComponentSource::Local(path) = &component.source {
                if component_binary::is_v2_component(path) {
                    // v2 component — check for binary (optional for template-only components)
                    let binary_path = component_binary::resolve_binary(path, &component.name);

                    if let Some(ref binary_path) = binary_path {
                        // v2 component: try cached descriptor first, then _meta/describe
                        let descriptor_result =
                            if let Some(cached) = component_binary::load_cached_descriptor(path) {
                                tracing::debug!(
                                    "using cached descriptor for {}/{}",
                                    component.organisation,
                                    component.name,
                                );
                                Ok(cached)
                            } else {
                                component_binary::describe_component(&binary_path).await
                            };
                        match descriptor_result {
                            Ok(descriptor) => {
                                let source = CommandSource::Local(
                                    path.canonicalize().context("get absolute path")?,
                                );
                                let mut registered = 0;
                        for method in &descriptor.methods {
                                    // Only register "commands/*" for `forest run`.
                                    // Hooks are invoked by forest release prepare, not `forest run`.
                                    if !method.name.starts_with("commands/") {
                                        continue;
                                    }

                                    // Strip "commands/" prefix for the CLI name
                                    let short_name = method
                                        .name
                                        .strip_prefix("commands/")
                                        .unwrap_or(&method.name);

                                    project.commands.insert(
                                        CommandName::Component {
                                            organisation: Some(component.organisation.clone()),
                                            name: component.name.clone(),
                                            source: source.clone(),
                                            command_name: short_name.to_string(),
                                        },
                                        crate::models::Command::ComponentBinary {
                                            binary_path: binary_path.clone(),
                                            method: method.name.clone(),
                                            description: method.description.clone(),
                                        },
                                    );
                                    registered += 1;
                                }
                                tracing::info!(
                                    "registered {} v2 commands from component {}/{}",
                                    registered,
                                    component.organisation,
                                    component.name,
                                );
                                continue;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "failed to describe v2 component {}/{}: {e}, falling back to v1",
                                    component.organisation,
                                    component.name,
                                );
                            }
                        }
                    } else if component_deno::is_deno_component(path) {
                        // Deno/TypeScript component
                        if let Some(entrypoint) = component_deno::resolve_entrypoint(path) {
                            let descriptor_result =
                                if let Some(cached) = component_deno::load_cached_descriptor(path) {
                                    tracing::debug!(
                                        "using cached descriptor for deno component {}/{}",
                                        component.organisation,
                                        component.name,
                                    );
                                    Ok(cached)
                                } else {
                                    component_deno::describe_deno_component(path, &entrypoint).await
                                };

                            match descriptor_result {
                                Ok(descriptor) => {
                                    let component_dir = path.canonicalize().context("get absolute path")?;
                                    let source = CommandSource::Local(component_dir.clone());
                                    let mut registered = 0;
                                    for method in &descriptor.methods {
                                        if !method.name.starts_with("commands/") {
                                            continue;
                                        }
                                        let short_name = method
                                            .name
                                            .strip_prefix("commands/")
                                            .unwrap_or(&method.name);

                                        project.commands.insert(
                                            CommandName::Component {
                                                organisation: Some(component.organisation.clone()),
                                                name: component.name.clone(),
                                                source: source.clone(),
                                                command_name: short_name.to_string(),
                                            },
                                            crate::models::Command::ComponentDeno {
                                                component_dir: component_dir.clone(),
                                                entrypoint: entrypoint.clone(),
                                                method: method.name.clone(),
                                                description: method.description.clone(),
                                            },
                                        );
                                        registered += 1;
                                    }
                                    tracing::info!(
                                        "registered {} deno commands from component {}/{}",
                                        registered,
                                        component.organisation,
                                        component.name,
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "failed to describe deno component {}/{}: {e}",
                                        component.organisation,
                                        component.name,
                                    );
                                }
                            }
                        }
                    } else {
                        // Template-only component (no binary, no deno) — participates in
                        // release prepare via templates, but has no `forest run` commands.
                        tracing::info!(
                            "template-only component {}/{} (no binary, templates only)",
                            component.organisation,
                            component.name,
                        );
                    }
                    continue; // v2 component handled — don't fall through to v1
                }
            }

            // v1/legacy: register commands from forest.component.toml (if any)
            tracing::trace!("found commands: {}", component.commands.len());

            for (command_name, command) in component.commands {
                project.commands.insert(
                    crate::models::CommandName::Component {
                        organisation: Some(component.organisation.clone()),
                        name: component.name.clone(),
                        source: match &component.source {
                            CacheComponentSource::Versioned(version) => {
                                CommandSource::Versioned(version.to_string())
                            }
                            CacheComponentSource::Local(path) => CommandSource::Local(
                                path.canonicalize().context("get absolute path")?,
                            ),
                            CacheComponentSource::Unknown => {
                                anyhow::bail!("a component source cannot be unknown")
                            }
                        },
                        command_name,
                    },
                    match command {
                        CacheComponentCommand::Inline(items) => {
                            crate::models::Command::Inline(items)
                        }
                        CacheComponentCommand::Script(name) => crate::models::Command::Script(name),
                    },
                );
            }
        }

        Ok(project)
    }

    async fn get_project_components(
        &self,
        project: &Project,
    ) -> anyhow::Result<Vec<CacheComponent>> {
        let components = self
            .component_service
            .get_components_project(project.clone())
            .await
            .inspect_err(|e| tracing::warn!("failed to get project components: {e:#}"))?;

        let mut project_components = Vec::new();
        for component in components.iter() {
            for dep in project.dependencies.dependencies.iter() {
                tracing::trace!(
                    name = dep.name,
                    organisation = dep.organisation,
                    "found component"
                );

                if dep.organisation == component.organisation && dep.name == component.name {
                    match &dep.dependency_type {
                        DependencyType::Versioned(version) if *version == component.version.to_string() => {
                            tracing::trace!(
                                name = dep.name,
                                organisation = dep.organisation,
                                "adding versioned component"
                            );
                            project_components.push(component.clone())
                        }
                        DependencyType::Local(path) if path == &component.path => {
                            tracing::trace!(
                                name = dep.name,
                                organisation = dep.organisation,
                                "adding local component"
                            );
                            project_components.push(component.clone())
                        }

                        // Ignoring items that don't match projects
                        DependencyType::Versioned(_) => continue,
                        DependencyType::Local(_) => continue,
                    }
                }
            }
        }

        Ok(project_components)
    }

    async fn parse_project_file(&self) -> Result<Project, anyhow::Error> {
        let current_dir =
            std::env::current_dir().context("current project dir is required for a project")?;
        let (project_file_path, project_file_content) = self.find_project_file(current_dir).await?;
        let mut project_file: ForestProject =
            toml::from_str(&project_file_content).context("parse file as toml")?;
        let raw: toml::Value =
            toml::from_str(&project_file_content).context("parse file as raw toml")?;
        project_file.raw = Some(raw);

        let mut project: Project = project_file.try_into().context("parse project")?;

        project.path = project_file_path
            .parent()
            .context("get parent for project file path")?
            .to_path_buf();

        Ok(project)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn find_project_file(
        &self,
        current_dir: std::path::PathBuf,
    ) -> anyhow::Result<(PathBuf, String)> {
        let mut dir_path = current_dir;

        loop {
            match self.get_project_file(&dir_path).await? {
                Some(output) => return Ok(output),
                None => {
                    if !dir_path.pop() {
                        anyhow::bail!("failed to find forest.toml in project");
                    }
                }
            }
        }
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn get_project_file(&self, dir: &Path) -> anyhow::Result<Option<(PathBuf, String)>> {
        let file_path = dir.join(FOREST_PROJECT_CUE_FILE);
        if file_path.exists() {
            // 1. Transform cue into toml
            let mut cmd = tokio::process::Command::new("cue");
            cmd.arg("export")
                .arg(&file_path)
                .arg("--out")
                .arg("toml");

            // Pass CUE_REGISTRY if set (enables module imports from OCI registry)
            if let Ok(registry) = std::env::var("CUE_REGISTRY") {
                cmd.env("CUE_REGISTRY", registry);
            }

            let output = cmd.output().await?;

            let stderr =
                std::string::String::from_utf8(output.stderr).context("interpret stderr")?;

            if !output.status.success() {
                anyhow::bail!("failed to evaluate {}: {}", file_path.display(), stderr.trim());
            }

            let output = std::string::String::from_utf8(output.stdout)
                .context("convert cue into native format (toml)")?;

            tracing::trace!("output: (stdout: {:?}, stderr: {:?})", output, stderr);

            return Ok(Some((file_path, output)));
        }
        let file_path = dir.join(FOREST_PROJECT_NICKEL_FILE);
        if file_path.exists() {
            // 1. Transform cue into toml
            let output = tokio::process::Command::new("nickel")
                .arg("export")
                .arg(&file_path)
                .arg("--format")
                .arg("toml")
                .output()
                .await?;

            let stderr =
                std::string::String::from_utf8(output.stderr).context("interpret stderr")?;
            let output = std::string::String::from_utf8(output.stdout)
                .context("convert nickel (ncl) into native format (toml)")?;

            tracing::debug!("output: (stdout: {:?}, stderr: {:?})", output, stderr);

            return Ok(Some((file_path, output)));
        }
        let file_path = dir.join(FOREST_PROJECT_YAML_FILE);
        if file_path.exists() {
            // 1. Transform cue into toml
            let output = tokio::process::Command::new("cue")
                .arg("export")
                .arg(&file_path)
                .arg("--out")
                .arg("toml")
                .output()
                .await?;

            let stderr =
                std::string::String::from_utf8(output.stderr).context("interpret stderr")?;
            let output = std::string::String::from_utf8(output.stdout)
                .context("convert yaml into native format (toml)")?;

            tracing::debug!("output: (stdout: {:?}, stderr: {:?})", output, stderr);

            return Ok(Some((file_path, output)));
        }

        let file_path = dir.join(FOREST_PROJECT_TOML_FILE);
        if file_path.exists() {
            let file_content = tokio::fs::read_to_string(&file_path)
                .await
                .context(format!("failed to read file: {}", file_path.display()))?;

            return Ok(Some((file_path, file_content)));
        }

        tracing::debug!("project file doesn't exist");
        return Ok(None);
    }
}

pub trait ProjectParserState {
    fn project_parser(&self) -> ProjectParser;
}

impl ProjectParserState for State {
    fn project_parser(&self) -> ProjectParser {
        ProjectParser {
            component_service: self.components_service(),
            temp: self.temp_directories(),
        }
    }
}

impl TryFrom<ForestProject> for Project {
    type Error = anyhow::Error;

    fn try_from(value: ForestProject) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.project.name,
            organisation: value.project.organisation,
            dependencies: crate::models::Dependencies {
                dependencies: value
                    .dependencies
                    .into_iter()
                    .map(|(entry, dep)| {
                        let version = match &dep {
                            Dependency::String(version) => version,
                            Dependency::Versioned(project_dependency) => {
                                &project_dependency.version
                            }
                            Dependency::Local(dep) => {
                                let (organisation, name) =
                                    entry.split_once("/").unwrap_or_else(|| ("forest", &entry));

                                return Ok(crate::models::Dependency {
                                    name: name.into(),
                                    organisation: organisation.into(),
                                    dependency_type: crate::models::DependencyType::Local(
                                        dep.path.clone(),
                                    ),
                                });
                            }
                        };

                        let (organisation, name) =
                            entry.split_once("/").unwrap_or_else(|| ("forest", &entry));

                        Ok(crate::models::Dependency {
                            name: name.into(),
                            organisation: organisation.into(),
                            dependency_type: crate::models::DependencyType::Versioned(
                                version.clone(),
                            ),
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?,
            },

            commands: value
                .commands
                .into_iter()
                .map(|(name, command)| {
                    (
                        CommandName::Project { command_name: name },
                        match command {
                            Command::Inline(items) => crate::models::Command::Inline(items),
                            Command::Script(script) => crate::models::Command::Script(script),
                        },
                    )
                })
                .collect(),
            path: PathBuf::default(),
            other: value
                .raw
                .map(ProjectValue::try_from)
                .transpose()?
                .unwrap_or_default(),
        })
    }
}

impl TryFrom<toml::Value> for ProjectValue {
    type Error = anyhow::Error;

    fn try_from(value: toml::Value) -> Result<Self, Self::Error> {
        let item = match value {
            toml::Value::String(s) => Self::String(s),
            toml::Value::Integer(i) => Self::Integer(i),
            toml::Value::Float(f) => Self::Decimal(f),
            toml::Value::Boolean(b) => Self::Bool(b),
            toml::Value::Datetime(datetime) => Self::String(datetime.to_string()),
            toml::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(ProjectValue::try_from)
                    .collect::<anyhow::Result<Vec<_>>>()?,
            ),
            toml::Value::Table(map) => Self::Map(
                map.into_iter()
                    .map(|(k, v)| Ok((k, ProjectValue::try_from(v)?)))
                    .collect::<anyhow::Result<HashMap<_, _>>>()?,
            ),
        };

        Ok(item)
    }
}
