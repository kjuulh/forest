package terraform_service

import "forest.sh/forest/sdk@v0"

// --- Input spec ---
// Note: `environment` is NOT in the spec — it comes from the deployment
// context (forest/config.json `env` field) and is available to terraform
// via `local.full_config.env`.
#Spec: sdk.#ForestSpec & {
	name:     string & =~"^[a-z][a-z0-9-]*$"
	replicas: int & >=1 & <=100 | *1
	ports: [...#Port]
	health_checks?: #HealthCheck
	env_vars: [...#EnvVar]
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
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": sdk.#ForestHook & {
		prepare: {
			description: "Generate Terraform files for deployment"
			input: {}
			output: {
				manifests: [...string]
			}
		}
		release: {
			description: "Apply Terraform configuration"
			input: {
				release_id: string
			}
			output: {}
		}
		rollback: {
			description: "Roll back Terraform state"
			input: {
				release_id:      string
				target_revision: string | *""
			}
		}
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

#EnvVar: {
	key:   string
	value: string
}
