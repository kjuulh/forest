#[allow(dead_code)]
mod forestgen;

use forestgen::*;

struct EcsCommands;

impl CommandHandler for EcsCommands {
    fn prepare(
        &self,
        spec: &Spec,
        _input: PrepareInput,
    ) -> Result<PrepareOutput, forest_sdk::Error> {
        println!(
            "Preparing ECS task definition for service '{}' (image: {}, cpu: {:?}, memory: {:?})",
            spec.name, spec.image, spec.cpu, spec.memory
        );

        Ok(PrepareOutput {})
    }

    fn status(&self, spec: &Spec, _input: StatusInput) -> Result<StatusOutput, forest_sdk::Error> {
        println!("Checking status for service '{}'", spec.name);

        Ok(StatusOutput {
            running: spec.replicas,
            desired: spec.replicas,
            healthy: true,
        })
    }
}

struct EcsDeploymentHooks;

impl ForestDeploymentHookHandler for EcsDeploymentHooks {
    fn prepare(
        &self,
        spec: &Spec,
        _input: ForestDeploymentPrepareInput,
    ) -> Result<ForestDeploymentPrepareOutput, forest_sdk::Error> {
        println!(
            "Deployment prepare: generating manifests for '{}'",
            spec.name
        );

        Ok(ForestDeploymentPrepareOutput {})
    }

    fn release(
        &self,
        spec: &Spec,
        input: ForestDeploymentReleaseInput,
    ) -> Result<ForestDeploymentReleaseOutput, forest_sdk::Error> {
        println!(
            "Deploying '{}' to ECS with release_id={}",
            spec.name, input.release_id
        );

        Ok(ForestDeploymentReleaseOutput {})
    }

    fn rollback(
        &self,
        spec: &Spec,
        input: ForestDeploymentRollbackInput,
    ) -> Result<(), forest_sdk::Error> {
        println!(
            "Rolling back '{}' in {:?} to release_id={}",
            input.name, input.environment, input.release_id
        );

        let _ = spec;

        Ok(())
    }
}

fn main() {
    let router = ComponentRouter::new(EcsCommands, EcsDeploymentHooks);
    forest_sdk::run_once(&router);
}
