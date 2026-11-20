use forest_sdk::{
    CallContext, ComponentService, Error, MethodDescriptor, MethodKind, RequiredTool,
};

use crate::Toolchain;

const BUILD_METHOD: &str = "commands/build";

pub struct BuildComponent {
    toolchain: Toolchain,
}

impl BuildComponent {
    pub fn new(toolchain: Toolchain) -> Self {
        Self { toolchain }
    }

    fn description(&self) -> String {
        format!(
            "Build this component with {}",
            match self.toolchain {
                Toolchain::Rust => "cargo",
                Toolchain::Golang => "go",
                Toolchain::Docker => "docker",
            }
        )
    }
}

impl ComponentService<serde_json::Value> for BuildComponent {
    fn call(
        &self,
        method: &str,
        _spec: &serde_json::Value,
        _input: serde_json::Value,
        context: &CallContext,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, Error>> + Send {
        let toolchain = self.toolchain;
        let method = method.to_string();
        let work_dir = context
            .work_dir
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| ".".to_string());

        async move {
            if method != BUILD_METHOD {
                return Err(Error::MethodNotFound(method));
            }
            let summary = crate::run_build(toolchain, std::path::Path::new(&work_dir))
                .await
                .map_err(|e| Error::Handler(format!("{e:#}").into()))?;
            serde_json::to_value(summary).map_err(Error::from)
        }
    }

    fn methods(&self) -> Vec<MethodDescriptor> {
        vec![MethodDescriptor {
            name: BUILD_METHOD.to_string(),
            kind: MethodKind::Command,
            description: Some(self.description()),
        }]
    }

    fn streaming_methods(&self) -> Vec<String> {
        vec![BUILD_METHOD.to_string()]
    }

    fn requires(&self) -> Vec<RequiredTool> {
        let mut tools = vec![RequiredTool {
            name: "cue".to_string(),
            hint: Some("Install cue — see https://cuelang.org/docs/install/".to_string()),
        }];
        match self.toolchain {
            Toolchain::Rust => tools.push(RequiredTool {
                name: "cargo".to_string(),
                hint: Some("Install the Rust toolchain — see https://rustup.rs".to_string()),
            }),
            Toolchain::Golang => tools.push(RequiredTool {
                name: "go".to_string(),
                hint: Some("Install Go — see https://go.dev/doc/install".to_string()),
            }),
            Toolchain::Docker => tools.push(RequiredTool {
                name: "docker".to_string(),
                hint: Some(
                    "Install Docker with buildx — see https://docs.docker.com/get-docker/"
                        .to_string(),
                ),
            }),
        }
        tools
    }
}

pub fn serve(toolchain: Toolchain) {
    forest_sdk::run_once(&BuildComponent::new(toolchain));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_declares_cargo_and_cue() {
        let c = BuildComponent::new(Toolchain::Rust);
        let names: Vec<_> = c.requires().into_iter().map(|t| t.name).collect();
        assert!(names.contains(&"cargo".to_string()));
        assert!(names.contains(&"cue".to_string()));
        assert!(!names.contains(&"go".to_string()));
    }

    #[test]
    fn build_is_the_only_method_and_streams() {
        let c = BuildComponent::new(Toolchain::Docker);
        let methods = c.methods();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, BUILD_METHOD);
        assert_eq!(c.streaming_methods(), vec![BUILD_METHOD.to_string()]);
    }
}
