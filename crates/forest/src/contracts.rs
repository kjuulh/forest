//! Forest contract system.
//!
//! A **contract** is a forest-owned hook topic (e.g., `forest/deployment`) that
//! defines a set of lifecycle hooks. Contracts are derived from the project's
//! dependencies — if any dependency implements `forest/deployment` hooks, the
//! deployment contract is available.
//!
//! Contracts are:
//! - Forest-only (`forest/*` topics)
//! - Derived from dependencies — no explicit enablement needed
//! - If `forest release` is invoked and no dependencies implement deployment,
//!   an error is returned

use crate::models::Project;
use crate::services::component_binary;

/// Known forest contract topics.
pub const CONTRACT_DEPLOYMENT: &str = "forest/deployment";
pub const CONTRACT_OBSERVABILITY: &str = "forest/observability";
pub const CONTRACT_SECURITY: &str = "forest/security";

/// All known contract topics.
pub const ALL_CONTRACTS: &[&str] = &[
    CONTRACT_DEPLOYMENT,
    CONTRACT_OBSERVABILITY,
    CONTRACT_SECURITY,
];

/// Contracts available in a project, derived from its dependencies.
#[derive(Debug, Clone)]
pub struct EnabledContracts {
    topics: Vec<String>,
}

impl EnabledContracts {
    /// Derive available contracts from a project's resolved dependencies.
    ///
    /// Scans each dependency's component descriptor for `forest/*` hook topics.
    /// A contract is enabled if at least one dependency implements it.
    pub async fn from_project_dependencies(project: &Project) -> Self {
        let mut topics_set = std::collections::BTreeSet::new();

        for component_ref in project.dependencies.get_components() {
            let path = match &component_ref.source {
                crate::models::ComponentSource::Local(p) => p.clone(),
                crate::models::ComponentSource::Versioned(_) => {
                    // For versioned deps, check the cache
                    let cache_dir = match dirs::cache_dir() {
                        Some(d) => d
                            .join("forest")
                            .join("components")
                            .join(&component_ref.organisation)
                            .join(&component_ref.name),
                        None => continue,
                    };
                    // Find latest version dir
                    match std::fs::read_dir(&cache_dir) {
                        Ok(entries) => {
                            let mut latest = None;
                            for entry in entries.flatten() {
                                if entry.path().is_dir() {
                                    latest = Some(entry.path());
                                }
                            }
                            match latest {
                                Some(p) => p,
                                None => continue,
                            }
                        }
                        Err(_) => continue,
                    }
                }
            };

            if !component_binary::is_v2_component(&path) {
                continue;
            }

            // Try to load cached descriptor (fast path — works for both binary and deno)
            if let Some(descriptor) = component_binary::load_cached_descriptor(&path) {
                for topic in component_contracts(&descriptor) {
                    topics_set.insert(topic);
                }
                continue;
            }

            // Also check deno cached descriptor
            if let Some(descriptor) =
                crate::services::component_deno::load_cached_descriptor(&path)
            {
                for topic in component_contracts(&descriptor) {
                    topics_set.insert(topic);
                }
                continue;
            }

            // Try to describe from binary
            if let Some(binary_path) =
                component_binary::resolve_binary(&path, &component_ref.name)
            {
                if let Ok(descriptor) = component_binary::describe_component(&binary_path).await {
                    for topic in component_contracts(&descriptor) {
                        topics_set.insert(topic);
                    }
                }
            }

            // Try to describe from deno
            if crate::services::component_deno::is_deno_component(&path) {
                if let Some(entrypoint) =
                    crate::services::component_deno::resolve_entrypoint(&path)
                {
                    if let Ok(descriptor) =
                        crate::services::component_deno::describe_deno_component(&path, &entrypoint)
                            .await
                    {
                        for topic in component_contracts(&descriptor) {
                            topics_set.insert(topic);
                        }
                    }
                }
            }
        }

        Self {
            topics: topics_set.into_iter().collect(),
        }
    }

    /// Check if a specific contract topic is enabled.
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
                "no dependencies implement the '{topic}' contract.\n\
                 Add a component that implements {topic} hooks to your dependencies in forest.cue.\n\
                 Example:\n  dependencies: {{\n    \"forest-contrib/terraform-service\": path: \"...\"\n  }}"
            )
        }
    }
}

/// Extract which contract topics a component implements, based on its descriptor.
///
/// Only returns `forest/*` topics (contracts), not custom hook topics.
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
    fn test_component_contracts() {
        let descriptor = forest_sdk::ComponentDescriptor {
            protocol_version: "1.1".into(),
            methods: vec![
                forest_sdk::MethodInfo {
                    name: "commands/prepare".into(),
                    kind: "command".into(),
                    topic: None,
                    description: None,
                },
                forest_sdk::MethodInfo {
                    name: "hooks/forest/deployment/prepare".into(),
                    kind: "hook".into(),
                    topic: Some("forest/deployment".into()),
                    description: None,
                },
                forest_sdk::MethodInfo {
                    name: "hooks/forest/observability/configure_monitoring".into(),
                    kind: "hook".into(),
                    topic: Some("forest/observability".into()),
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
        assert_eq!(contracts, vec!["forest/deployment", "forest/observability"]);
        // custom/something is NOT a contract
    }
}
