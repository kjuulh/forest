package ecs_service

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// =============================================================================
// ECS service component — deploys a single Fargate service to the platform
// ECS cluster (`infrastructure-platform`) behind the shared HTTPS ALB
// (`shared-alb`). Targeted at the `forest/terraform@1` destination type.
//
// `environment` is NOT in the spec; it comes from the deployment context
// (`forest/config.json` `env`) and is read in main.tf as
// `local.full_config.env`.
// =============================================================================

#Spec: sdk.#ForestSpec & {
	// ECS service + task family name.
	name: string & =~"^[a-z][a-z0-9-]*$"

	// Container image (typically `ghcr.io/<org>/<repo>:<tag>`).
	image: string

	// Optional container command (overrides the image's default ENTRYPOINT args).
	command?: [...string]

	// Fargate task sizing. Strings because ECS expects them as strings.
	cpu:    string | *"256"
	memory: string | *"512"

	// Desired ECS service replicas.
	replicas: int & >=1 & <=100 | *1

	// Single container port. The container listens here, and the ALB target
	// group routes traffic to it.
	port: int & >0 & <=65535

	// ALB host header that routes to this service. The hostname must already
	// resolve to the shared ALB (Cloudflare CNAME). The empty list `[]`
	// matches any hostname (last-resort default).
	host_headers: [...string]

	// ALB listener-rule priority. Must be unique across the listener.
	// See infrastructure-platform/.tf for what's already in use.
	priority: int & >=1 & <=50000

	// Plain-text environment variables.
	env_vars: {[string]: string} | *{}

	// Names of keys inside the `${env}/${name}/env` Secrets Manager secret
	// that should be exposed to the container. The component does NOT create
	// the secret — only references it.
	secrets: [...string] | *[]

	// HTTP health-check path. Used both by ECS (no native check on Fargate
	// — relied on ALB target health) and the ALB target group.
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
