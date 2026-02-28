#[allow(dead_code)]
mod forestgen;

use forestgen::*;

struct ServiceCommands;

impl CommandHandler for ServiceCommands {
    fn build(&self, spec: &Spec, input: BuildInput) -> Result<BuildOutput, forest_sdk::Error> {
        let image = format!("{}:{}", spec.image, input.tag);
        println!("Building container image: {image}");
        println!("  Base image: {}", spec.image);
        println!("  Ports: {:?}", spec.ports.iter().map(|p| p.port).collect::<Vec<_>>());
        println!(
            "  Resources: cpu={}/{}, memory={}/{}",
            spec.resources.requests.cpu,
            spec.resources.limits.cpu,
            spec.resources.requests.memory,
            spec.resources.limits.memory,
        );
        Ok(BuildOutput { image })
    }

    fn validate(
        &self,
        spec: &Spec,
        _input: ValidateInput,
    ) -> Result<ValidateOutput, forest_sdk::Error> {
        let mut messages = Vec::new();
        let mut valid = true;

        if spec.ports.is_empty() {
            messages.push("Warning: no ports defined".into());
        }

        for port in &spec.ports {
            if port.port < 1 || port.port > 65535 {
                valid = false;
                messages.push(format!("Invalid port {}: must be 1-65535", port.port));
            }
        }

        if spec.replicas < 1 {
            valid = false;
            messages.push("Replicas must be at least 1".into());
        }

        if valid {
            messages.push("Spec is valid".into());
        }

        println!("Validation result: valid={valid}, messages={messages:?}");
        Ok(ValidateOutput { valid, messages })
    }

    fn status(&self, spec: &Spec, _input: StatusInput) -> Result<StatusOutput, forest_sdk::Error> {
        println!("Checking status for service '{}'", spec.name);
        println!("  Desired replicas: {}", spec.replicas);
        Ok(StatusOutput {
            running: spec.replicas,
            desired: spec.replicas,
            healthy: true,
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
        println!("Preparing Kubernetes manifests for '{}'", spec.name);

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

        println!("  Generated {} manifest(s)", manifests.len());
        Ok(ForestDeploymentPrepareOutput { manifests })
    }

    fn release(
        &self,
        spec: &Spec,
        input: ForestDeploymentReleaseInput,
    ) -> Result<ForestDeploymentReleaseOutput, forest_sdk::Error> {
        println!(
            "Deploying '{}' (image: {}) with release_id={}",
            spec.name, spec.image, input.release_id
        );
        println!("  Replicas: {}", spec.replicas);
        println!(
            "  Health checks: liveness={}, readiness={}",
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
            "Rolling back '{}' to release_id={}",
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
