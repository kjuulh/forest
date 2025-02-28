use std::{collections::BTreeMap, fmt::Debug, path::PathBuf};

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Context {
    pub project: Project,
    pub plan: Option<Plan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub name: String,
    pub templates: Option<Templates>,
    pub scripts: Option<Scripts>,
}

impl TryFrom<KdlDocument> for Plan {
    type Error = anyhow::Error;

    fn try_from(value: KdlDocument) -> Result<Self, Self::Error> {
        let plan_section = value.get("plan").ok_or(anyhow::anyhow!(
            "forest.kdl plan file must have a plan object"
        ))?;

        let plan_children = plan_section
            .children()
            .ok_or(anyhow::anyhow!("a forest plan must have children"))?;

        Ok(Self {
            name: plan_children
                .get_arg("name")
                .and_then(|n| match n {
                    KdlValue::String(s) => Some(s),
                    _ => None,
                })
                .cloned()
                .ok_or(anyhow::anyhow!("a forest kuddle plan must have a name"))?,
            templates: plan_children
                .get("templates")
                .map(|t| t.try_into())
                .transpose()?,
            scripts: plan_children
                .get("scripts")
                .map(|m| m.try_into())
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ProjectPlan {
    Local { path: PathBuf },
    NoPlan,
}

impl TryFrom<&KdlNode> for ProjectPlan {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        let Some(children) = value.children() else {
            return Ok(Self::NoPlan);
        };

        if let Some(local) = children.get_arg("local") {
            return Ok(Self::Local {
                path: local
                    .as_string()
                    .map(|l| l.to_string())
                    .ok_or(anyhow::anyhow!("local must have an arg with a valid path"))?
                    .into(),
            });
        }

        Ok(Self::NoPlan)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum GlobalVariable {
    Map(BTreeMap<String, GlobalVariable>),
    String(String),
    Float(f64),
    Integer(i128),
    Bool(bool),
}

impl TryFrom<&KdlDocument> for GlobalVariable {
    type Error = anyhow::Error;

    fn try_from(value: &KdlDocument) -> Result<Self, Self::Error> {
        let nodes = value.nodes();
        if nodes.is_empty() {
            return Ok(Self::Map(BTreeMap::default()));
        }

        let mut items = BTreeMap::new();
        for node in nodes {
            let name = node.name().value();
            if let Some(children) = node.children() {
                let val: GlobalVariable = children.try_into()?;
                items.insert(name.into(), val);
            } else if let Some(entry) = node.entries().first() {
                items.insert(name.into(), entry.value().try_into()?);
            } else {
                items.insert(name.into(), GlobalVariable::Map(BTreeMap::default()));
            }
        }

        Ok(GlobalVariable::Map(items))
    }
}

impl TryFrom<&KdlValue> for GlobalVariable {
    type Error = anyhow::Error;

    fn try_from(value: &KdlValue) -> Result<Self, Self::Error> {
        if let Some(value) = value.as_string() {
            return Ok(Self::String(value.to_string()));
        }

        if let Some(value) = value.as_integer() {
            return Ok(Self::Integer(value));
        }

        if let Some(value) = value.as_float() {
            return Ok(Self::Float(value));
        }

        if let Some(value) = value.as_bool() {
            return Ok(Self::Bool(value));
        }

        anyhow::bail!("value is not supported by global variables")
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Global {
    #[serde(flatten)]
    items: BTreeMap<String, GlobalVariable>,
}

impl From<&Global> for minijinja::Value {
    fn from(value: &Global) -> Self {
        Self::from_serialize(&value.items)
    }
}

impl TryFrom<&KdlNode> for Global {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        let mut global = Global::default();
        let Some(item) = value.children() else {
            return Ok(global);
        };

        for node in item.nodes() {
            let name = node.name().value();
            if let Some(children) = node.children() {
                let val: GlobalVariable = children.try_into()?;
                global.items.insert(name.into(), val);
            } else if let Some(entry) = node.entries().first() {
                global.items.insert(name.into(), entry.value().try_into()?);
            }
        }

        Ok(global)
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub enum TemplateType {
    #[default]
    Jinja2,
}

#[derive(Debug, Clone, Serialize)]
pub struct Templates {
    #[serde(rename = "type")]
    pub ty: TemplateType,
    pub path: String,
    pub output: PathBuf,
}

impl Default for Templates {
    fn default() -> Self {
        Self {
            ty: TemplateType::default(),
            path: "./templates/*.jinja2".into(),
            output: "output/".into(),
        }
    }
}

impl TryFrom<&KdlNode> for Templates {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        let mut templates = Templates::default();

        for entry in value.entries() {
            let Some(name) = entry.name() else { continue };
            match name.value() {
                "type" => {
                    let Some(val) = entry.value().as_string() else {
                        anyhow::bail!("type is not a valid string")
                    };

                    match val.to_lowercase().as_str() {
                        "jinja2" => templates.ty = TemplateType::Jinja2,
                        e => {
                            anyhow::bail!(
                                "failed to find a template matching the required type: {}, only 'jinja2' is supported",
                                e
                            );
                        }
                    }
                }
                "path" => {
                    let Some(val) = entry.value().as_string() else {
                        anyhow::bail!("failed to parse path as a valid string")
                    };

                    templates.path = val.to_string();
                }
                "output" => {
                    let Some(val) = entry.value().as_string() else {
                        anyhow::bail!("failed to parse val as a valid string")
                    };

                    templates.output = PathBuf::from(val);
                }
                _ => continue,
            }
        }

        Ok(templates)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Script {
    Shell {},
}

impl TryFrom<&KdlNode> for Script {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        Ok(Self::Shell {})
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Scripts {
    #[serde(flatten)]
    pub items: BTreeMap<String, Script>,
}

impl TryFrom<&KdlNode> for Scripts {
    type Error = anyhow::Error;

    fn try_from(value: &KdlNode) -> Result<Self, Self::Error> {
        let val = Self {
            items: {
                let mut out = BTreeMap::default();
                if let Some(children) = value.children() {
                    for entry in children.nodes() {
                        let name = entry.name().value();
                        let val = entry.try_into()?;

                        out.insert(name.to_string(), val);
                    }

                    out
                } else {
                    out
                }
            },
        };

        Ok(val)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub name: String,
    pub description: Option<String>,
    pub plan: Option<ProjectPlan>,
    pub global: Global,
    pub templates: Option<Templates>,
    pub scripts: Option<Scripts>,
}

impl TryFrom<KdlDocument> for Project {
    type Error = anyhow::Error;

    fn try_from(value: KdlDocument) -> Result<Self, Self::Error> {
        let project_section = value.get("project").ok_or(anyhow::anyhow!(
            "forest.kdl project file must have a project object"
        ))?;

        let project_children = project_section
            .children()
            .ok_or(anyhow::anyhow!("a forest project must have children"))?;

        let project_plan: Option<ProjectPlan> = if let Some(project) = project_children.get("plan")
        {
            Some(project.try_into()?)
        } else {
            None
        };

        let global: Option<Global> = if let Some(global) = project_children.get("global") {
            Some(global.try_into()?)
        } else {
            None
        };

        Ok(Self {
            name: project_children
                .get_arg("name")
                .and_then(|n| match n {
                    KdlValue::String(s) => Some(s),
                    _ => None,
                })
                .cloned()
                .ok_or(anyhow::anyhow!("a forest kuddle project must have a name"))?,
            description: project_children
                .get_arg("description")
                .and_then(|n| match n {
                    KdlValue::String(s) => Some(s.trim().to_string()),
                    _ => None,
                }),
            plan: project_plan,
            global: global.unwrap_or_default(),
            templates: project_children
                .get("templates")
                .map(|t| t.try_into())
                .transpose()?,
            scripts: project_children
                .get("scripts")
                .map(|m| m.try_into())
                .transpose()?,
        })
    }
}
