package ecs_service

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

#Spec: sdk.#ForestSpec & {
	name: string & =~"^[a-z][a-z0-9-]*$"

	image: string

	command?: [...string]

	cpu:    string | *"256"
	memory: string | *"512"

	replicas: int & >=1 & <=100 | *1

	port: int & >0 & <=65535

	host_headers: [...string]

	priority: int & >=1 & <=50000

	env_vars: {[string]: string} | *{}

	secrets: [...string] | *[]

	health_check_path: string | *"/"
}

#Commands: sdk.#ForestCommands & {
	prepare: {
		description: "Generate Terraform configuration files"
		input: {}
		output: {
			manifests: [...string]
		}
	}
	status: {
		description: "Check ECS service status"
		input: {}
		output: {
			healthy: bool
		}
	}
	validate: {
		description: "Validate the spec"
		input: {}
		output: {
			valid:  bool
			errors: [...string]
		}
	}
}

#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Generate Terraform files for the ECS service"
		release: description:  "Apply Terraform configuration (creates / updates the ECS service)"
		rollback: description: "Roll back to a previous Terraform state"
	}
}
