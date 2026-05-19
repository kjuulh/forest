package terraform_service

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
// Note: `environment` is NOT in the spec — it comes from the deployment
// context (forest/config.json `env` field) and is available to terraform
// via `local.full_config.env`.
#Spec: sdk.#ForestSpec & {
	name:     string & =~"^[a-z][a-z0-9-]*$"
	replicas: int & >=1 & <=100 | *1
	ports: [...#Port]
	health_checks?: #HealthCheck
	env_vars: #EnvVars
}

// --- Commands ---
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
		description: "Validate Terraform configuration"
		input: {}
		output: {
			valid:  bool
			errors: [...string]
		}
	}
}

// --- Hooks ---
// Implements the forest/deployment contract.
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description: "Generate Terraform files for deployment"
		release: description: "Apply Terraform configuration"
		rollback: description: "Roll back Terraform state"
	}
}

// --- Types ---
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

#EnvVars: {
	[string]: string
}
