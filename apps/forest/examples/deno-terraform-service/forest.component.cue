package deno_terraform_service

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

#Spec: sdk.#ForestSpec & {
	name:     string & =~"^[a-z][a-z0-9-]*$"
	replicas: int & >=1 & <=100 | *1
	ports: [...#Port]
	health_checks?: #HealthCheck
	env_vars: [...#EnvVar]
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
		description: "Check deployment status"
		input: {}
		output: {
			healthy: bool
		}
	}
	validate: {
		description: "Validate configuration"
		input: {}
		output: {
			valid:  bool
			errors: [...string]
		}
	}
}

// Implements the forest/deployment contract.
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description: "Generate Terraform files for deployment"
		release: description: "Apply Terraform configuration"
		rollback: description: "Roll back Terraform state"
	}
}

#Port: {
	name:      string
	port:      int & >0 & <=65535
	external:  bool | *false
	subdomain?: string
}

#HealthCheck: {
	live?: {
		http?: {path: string, port: int}
		tcp?:  {port: int}
	}
}

#EnvVar: {
	key:   string
	value: string
}
