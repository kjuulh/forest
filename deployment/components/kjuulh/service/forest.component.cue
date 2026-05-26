package service

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
#Spec: sdk.#ForestSpec & {
	name:  string & =~"^[a-z][a-z0-9-]*$"
	image: string
	tag:   string | *"latest"
	host:  string
	port:  int

	// Container command and args (optional)
	command?: [...string]
	args?: [...string]

	// Environment variables passed directly to the container
	env_vars?: [string]: string

	// Port name used in the Service/Ingress (e.g. "http", "grpc")
	port_name: string | *"http"

	// Number of replicas (default: 1)
	replicas: int | *1

	// Health check binary and base URL override
	health_binary?: string
	health_host?:   string

	// Optional forage resource provisioning.
	// Names, namespaces, and accounts default from the service name.
	forage_postgresql?: {
		database_name?:    string   // defaults to service name
		secret_name?:      string   // defaults to "{name}-db-credentials"
		secret_namespace?: string
	}
	forage_nats?: {
		publish: [...{subject: string}]
		subscribe: [...{subject: string}]
		secret_name?: string
	}
	forage_s3?: {
		bucket_name?: string   // defaults to service name
		key_name?:    string
		secret_name?: string
		quotas?: {
			max_size?:    int
			max_objects?: int
		}
		permissions?: {
			read:  bool | *true
			write: bool | *true
			owner: bool | *false
		}
	}
}

// --- Commands ---
#Commands: sdk.#ForestCommands & {
	validate: {
		description: "Validate manifests"
		input: {}
		output: {
			valid:  bool
			errors: [...string]
		}
	}
	status: {
		description: "Check deployment status"
		input: {}
		output: {
			healthy: bool
		}
	}
	seal: {
		description: "Add or update a sealed secret key"
		input: {
			env:   string
			key:   string
			value: string
			cert:  string
		}
		output: {}
	}
}

// --- Hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Generate Kubernetes manifests with sealed secrets"
		release: description:  "Apply deployment"
		rollback: description: "Roll back deployment"
	}
}
