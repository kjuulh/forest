#[allow(dead_code)]
mod forestgen;

use forestgen::*;

struct ServiceCommands;

impl CommandHandler for ServiceCommands {
    fn build(&self, spec: &Spec, _input: BuildInput) -> Result<BuildOutput, forest_sdk::Error> {
        let binary = format!("target/release/{}", spec.name);
        println!("==> Compiling release binary for '{}'", spec.name);
        println!("  image: {}", spec.image);
        println!(
            "  ports: {:?}",
            spec.ports.iter().map(|p| p.port).collect::<Vec<_>>()
        );
        println!(
            "  resources: cpu={}/{}, memory={}/{}",
            spec.resources.requests.cpu,
            spec.resources.limits.cpu,
            spec.resources.requests.memory,
            spec.resources.limits.memory,
        );
        println!("==> Binary: {binary}");
        Ok(BuildOutput { binary })
    }

    fn validate(
        &self,
        spec: &Spec,
        _input: ValidateInput,
    ) -> Result<ValidateOutput, forest_sdk::Error> {
        let mut messages = Vec::new();
        let mut valid = true;

        // Check ports
        if spec.ports.is_empty() {
            messages.push("warning: no ports defined".into());
        }
        for port in &spec.ports {
            if port.port < 1 || port.port > 65535 {
                valid = false;
                messages.push(format!("error: invalid port {}: must be 1-65535", port.port));
            }
        }

        // Check replicas
        if spec.replicas < 1 {
            valid = false;
            messages.push("error: replicas must be >= 1".into());
        }

        // Check health check ports are defined
        let port_numbers: Vec<i64> = spec.ports.iter().map(|p| p.port).collect();
        if !port_numbers.contains(&spec.health_checks.liveness.port) {
            messages.push(format!(
                "warning: liveness probe port {} not in defined ports",
                spec.health_checks.liveness.port
            ));
        }
        if !port_numbers.contains(&spec.health_checks.readiness.port) {
            messages.push(format!(
                "warning: readiness probe port {} not in defined ports",
                spec.health_checks.readiness.port
            ));
        }

        // Check environment for duplicates
        let mut seen_keys = std::collections::HashSet::new();
        for env in &spec.environment {
            if !seen_keys.insert(&env.key) {
                messages.push(format!("warning: duplicate env var '{}'", env.key));
            }
        }

        // Check resource requests <= limits
        // (simplified: just report what we see)
        messages.push(format!(
            "info: resources cpu {}/{}, memory {}/{}",
            spec.resources.requests.cpu,
            spec.resources.limits.cpu,
            spec.resources.requests.memory,
            spec.resources.limits.memory,
        ));

        if valid {
            messages.push("ok: spec is valid".into());
        }

        println!("==> Validation: valid={valid}");
        for msg in &messages {
            println!("  {msg}");
        }
        Ok(ValidateOutput { valid, messages })
    }

    fn test(&self, spec: &Spec, _input: TestInput) -> Result<TestOutput, forest_sdk::Error> {
        println!("==> Running test suite for '{}'", spec.name);
        // In a real component, this would invoke `cargo test` or `cargo nextest run`
        // and parse the output. Here we simulate a passing test run.
        println!("  running unit tests...");
        println!("  running integration tests...");
        let output = TestOutput {
            passed: 12,
            failed: 0,
            total: 12,
        };
        println!(
            "==> Tests: {}/{} passed, {} failed",
            output.passed, output.total, output.failed
        );
        Ok(output)
    }

    fn docker_build(
        &self,
        spec: &Spec,
        input: DockerBuildInput,
    ) -> Result<DockerBuildOutput, forest_sdk::Error> {
        let registry = if input.registry.is_empty() {
            // Extract registry from image spec
            spec.image
                .rsplit_once('/')
                .map(|(reg, _)| reg.to_string())
                .unwrap_or_else(|| "registry.example.com".into())
        } else {
            input.registry.clone()
        };

        let image = format!("{}/{}:{}", registry, spec.name, input.tag);
        println!("==> Building Docker image: {image}");
        println!("  Dockerfile: .forest/Dockerfile (generated)");
        println!("  context: .");
        println!("  binary: target/release/{}", spec.name);
        println!(
            "  ports: {:?}",
            spec.ports.iter().map(|p| p.port).collect::<Vec<_>>()
        );
        println!("==> Image: {image}");
        Ok(DockerBuildOutput { image })
    }

    fn status(&self, spec: &Spec, _input: StatusInput) -> Result<StatusOutput, forest_sdk::Error> {
        println!("==> Status for '{}'", spec.name);

        let binary = format!("target/release/{}", spec.name);
        let binary_exists = std::path::Path::new(&binary).exists();
        println!(
            "  binary: {} ({})",
            binary,
            if binary_exists { "exists" } else { "not built" }
        );

        // Simulate git info (in real usage, this runs in the project dir)
        let git_branch = std::env::var("FOREST_GIT_BRANCH").unwrap_or_else(|_| "unknown".into());
        let git_commit = std::env::var("FOREST_GIT_COMMIT").unwrap_or_else(|_| "unknown".into());
        let git_dirty = std::env::var("FOREST_GIT_DIRTY")
            .map(|v| v == "true")
            .unwrap_or(false);

        println!("  branch: {git_branch}");
        println!("  commit: {git_commit}");
        println!("  dirty: {git_dirty}");
        println!("  replicas: {}", spec.replicas);
        println!("  image: {}", spec.image);

        Ok(StatusOutput {
            binary_exists,
            git_branch,
            git_commit,
            git_dirty,
        })
    }
}

struct DeploymentHooks;

impl ForestDeploymentHookHandler for DeploymentHooks {
    fn prepare(
        &self,
        spec: &Spec,
        _input: ForestDeploymentPrepareInput,
    ) -> Result<ForestDeploymentPrepareOutput, forest_sdk::Error> {
        println!("==> Preparing Kubernetes manifests for '{}'", spec.name);

        let deployment = format!(
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {}\nspec:\n  replicas: {}",
            spec.name, spec.replicas
        );

        let mut manifests = vec![deployment];

        let external_ports: Vec<_> = spec.ports.iter().filter(|p| p.external).collect();
        if !external_ports.is_empty() {
            let service = format!(
                "apiVersion: v1\nkind: Service\nmetadata:\n  name: {}\nspec:\n  type: ClusterIP",
                spec.name
            );
            manifests.push(service);
        }

        println!("  generated {} manifest(s)", manifests.len());
        Ok(ForestDeploymentPrepareOutput { manifests })
    }

    fn release(
        &self,
        spec: &Spec,
        input: ForestDeploymentReleaseInput,
    ) -> Result<ForestDeploymentReleaseOutput, forest_sdk::Error> {
        println!(
            "==> Deploying '{}' (image: {}) release_id={}",
            spec.name, spec.image, input.release_id
        );
        println!("  replicas: {}", spec.replicas);
        println!(
            "  health checks: liveness={}, readiness={}",
            spec.health_checks.liveness.path, spec.health_checks.readiness.path
        );
        Ok(ForestDeploymentReleaseOutput { deployed: true })
    }

    fn rollback(
        &self,
        spec: &Spec,
        input: ForestDeploymentRollbackInput,
    ) -> Result<(), forest_sdk::Error> {
        println!(
            "==> Rolling back '{}' to release_id={}",
            input.name, input.release_id
        );
        let _ = spec;
        Ok(())
    }
}

fn main() {
    let router = ComponentRouter::new(ServiceCommands, DeploymentHooks);
    forest_sdk::run_once(&router);
}
