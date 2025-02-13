use std::path::PathBuf;

use kdl::{KdlDocument, KdlNode, KdlValue};

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub description: Option<String>,
    pub plan: Option<ProjectPlan>,
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
        })
    }
}
