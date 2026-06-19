#[allow(dead_code)]
mod forestgen;

use forestgen::*;

struct Commands;
struct Deployment;

impl CommandHandler for Commands {
    async fn prepare(
        &self,
        _spec: &Spec,
        _input: PrepareInput,
    ) -> Result<PrepareOutput, forest_sdk::Error> {
        Ok(PrepareOutput { manifests: vec![] })
    }

    async fn status(
        &self,
        _spec: &Spec,
        _input: StatusInput,
    ) -> Result<StatusOutput, forest_sdk::Error> {
        Ok(StatusOutput { healthy: true })
    }

    async fn validate(
        &self,
        spec: &Spec,
        _input: ValidateInput,
    ) -> Result<ValidateOutput, forest_sdk::Error> {
        let mut errors = Vec::new();
        if spec.name.is_empty() {
            errors.push("name must not be empty".to_string());
        }
        Ok(ValidateOutput {
            valid: errors.is_empty(),
            errors,
        })
    }
}

impl ForestDeploymentHookHandler for Deployment {
    async fn prepare(
        &self,
        _spec: &Spec,
        _input: ForestDeploymentPrepareInput,
    ) -> Result<(), forest_sdk::Error> {
        Ok(())
    }

    async fn release(
        &self,
        spec: &Spec,
        _input: ForestDeploymentReleaseInput,
    ) -> Result<(), forest_sdk::Error> {
        eprintln!("terraform apply for '{}'", spec.name);
        Ok(())
    }

    async fn rollback(
        &self,
        spec: &Spec,
        _input: ForestDeploymentRollbackInput,
    ) -> Result<(), forest_sdk::Error> {
        eprintln!("terraform destroy for '{}'", spec.name);
        Ok(())
    }
}

fn main() {
    let router = ComponentRouter::new(Commands, Deployment);
    forest_sdk::run_once(&router);
}
