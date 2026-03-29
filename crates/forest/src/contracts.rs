//! Forest contract system.
//!
//! A **contract** is a published component (e.g., `forest/deployment`) that
//! defines hook signatures. Like Rust traits — the contract component is the
//! trait definition, and both the implementing component AND the consumer
//! project must depend on it for the hooks to be invoked.
//!
//! Resolution:
//! 1. Check the project's `dependencies:` for known contract components
//! 2. Check each component dependency's `dependencies:` for contracts too
//! 3. Both the project AND the component must have the contract in deps
//!    for hooks to fire

use crate::models::Project;

/// Known forest contract components.
/// These are the `org/name` keys as they appear in `dependencies:`.
pub const CONTRACT_DEPLOYMENT: &str = "forest/deployment";
pub const CONTRACT_OBSERVABILITY: &str = "forest/observability";
pub const CONTRACT_SECURITY: &str = "forest/security";

/// All known contract component names.
const ALL_CONTRACTS: &[&str] = &[
    CONTRACT_DEPLOYMENT,
    CONTRACT_OBSERVABILITY,
    CONTRACT_SECURITY,
];

pub fn is_contract(dep_key: &str) -> bool {
    ALL_CONTRACTS.contains(&dep_key)
}

/// Contracts available in a project, derived from its `dependencies:` field.
#[derive(Debug, Clone)]
pub struct EnabledContracts {
    topics: Vec<String>,
}

impl EnabledContracts {
    /// Derive available contracts from the project's dependencies.
    ///
    /// A contract is enabled if the contract component appears in the
    /// project's `dependencies:` (directly). The project must explicitly
    /// opt in — transitive deps through components are not enough.
    pub fn from_project_dependencies(project: &Project) -> Self {
        let mut topics = Vec::new();

        for dep in &project.dependencies.dependencies {
            let dep_key = format!("{}/{}", dep.organisation, dep.name);
            if is_contract(&dep_key) {
                topics.push(dep_key);
            }
        }

        topics.sort();
        Self { topics }
    }

    /// Check if a specific contract is enabled.
    pub fn is_enabled(&self, topic: &str) -> bool {
        self.topics.iter().any(|t| t == topic)
    }

    /// Get all enabled contract topics.
    pub fn topics(&self) -> &[String] {
        &self.topics
    }

    /// Returns true if any contracts are enabled.
    pub fn has_any(&self) -> bool {
        !self.topics.is_empty()
    }

    /// Require that a specific contract is available, or return a clear error.
    pub fn require(&self, topic: &str) -> anyhow::Result<()> {
        if self.is_enabled(topic) {
            Ok(())
        } else {
            anyhow::bail!(
                "the '{topic}' contract is not in your dependencies.\n\
                 Add it to your forest.cue:\n  \
                 dependencies: {{\n    \
                   \"{topic}\": path: \"path/to/{topic}\"\n  \
                 }}"
            )
        }
    }
}

/// Extract which contract topics a component implements, based on its descriptor.
///
/// Only returns `forest/*` topics, not custom hook topics.
pub fn component_contracts(descriptor: &forest_sdk::ComponentDescriptor) -> Vec<String> {
    let mut topics: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for method in &descriptor.methods {
        if method.kind == "hook" {
            if let Some(topic) = &method.topic {
                if topic.starts_with("forest/") {
                    topics.insert(topic.clone());
                }
            }
        }
    }

    topics.into_iter().collect()
}

impl std::fmt::Display for EnabledContracts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.topics.is_empty() {
            write!(f, "none")
        } else {
            write!(f, "{}", self.topics.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_project_with_contract_dep() {
        let project = Project {
            name: "test".into(),
            organisation: Some("myorg".into()),
            dependencies: crate::models::Dependencies {
                dependencies: vec![
                    crate::models::Dependency {
                        name: "deployment".into(),
                        organisation: "forest".into(),
                        dependency_type: crate::models::DependencyType::Local(
                            "../../components/forest/deployment".into(),
                        ),
                    },
                    crate::models::Dependency {
                        name: "terraform-service".into(),
                        organisation: "forest-contrib".into(),
                        dependency_type: crate::models::DependencyType::Local(
                            "../../components/forest-contrib/terraform-service".into(),
                        ),
                    },
                ],
            },
            commands: Default::default(),
            path: Default::default(),
            other: crate::models::ProjectValue::Map(Default::default()),
        };

        let contracts = EnabledContracts::from_project_dependencies(&project);
        assert!(contracts.is_enabled(CONTRACT_DEPLOYMENT));
        assert!(!contracts.is_enabled(CONTRACT_OBSERVABILITY));
    }

    #[test]
    fn test_from_project_without_contract() {
        let project = Project {
            name: "test".into(),
            organisation: Some("myorg".into()),
            dependencies: crate::models::Dependencies {
                dependencies: vec![crate::models::Dependency {
                    name: "terraform-service".into(),
                    organisation: "forest-contrib".into(),
                    dependency_type: crate::models::DependencyType::Local(
                        "../../components/forest-contrib/terraform-service".into(),
                    ),
                }],
            },
            commands: Default::default(),
            path: Default::default(),
            other: crate::models::ProjectValue::Map(Default::default()),
        };

        let contracts = EnabledContracts::from_project_dependencies(&project);
        assert!(!contracts.is_enabled(CONTRACT_DEPLOYMENT));
        assert!(!contracts.has_any());
    }

    #[test]
    fn test_component_contracts() {
        let descriptor = forest_sdk::ComponentDescriptor {
            protocol_version: "1.1".into(),
            methods: vec![
                forest_sdk::MethodInfo {
                    name: "hooks/forest/deployment/prepare".into(),
                    kind: "hook".into(),
                    topic: Some("forest/deployment".into()),
                    description: None,
                },
                forest_sdk::MethodInfo {
                    name: "hooks/custom/something".into(),
                    kind: "hook".into(),
                    topic: Some("custom/something".into()),
                    description: None,
                },
            ],
        };

        let contracts = component_contracts(&descriptor);
        assert_eq!(contracts, vec!["forest/deployment"]);
    }
}
