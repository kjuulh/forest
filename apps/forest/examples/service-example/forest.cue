package service_example

import (
	"forest.sh/forest/sdk@v0"
	tf "forest.sh/forest-contrib/terraform-service@v0:terraform_service"
)

project: sdk.#ForestProject & {
	name:         "forest-hello"
	organisation: "pworg2182261"
}

_destinationTypes: {
	terraform: "forest/terraform@1"
	forage:    "forage/containers@1"
}

#Terraform: {
	destination: string
	type:        "forest/terraform@1"
}
#Forage: {
	destination: string
	type:        "forage/containers/@1"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": path:                "../../components/forest/deployment"
	"forest-contrib/terraform-service": path: "../../components/forest-contrib/terraform-service"
}

"forest-contrib": "terraform-service": sdk.#ForestComponentUsage & {
	env: {
		dev: {
			destinations: [
				#Terraform & {destination: "infrastructure-dev.*"},
				#Forage & {destination: "forage/.*"},
			]
			config: {
				replicas: 2
				env_vars: RUST_LOG: "debug"
			}
		}

		staging: {
			destinations: [#Terraform & {destination: "infrastructure-staging.*"}]
			config: {
				replicas: 3
				env_vars: RUST_LOG: "info"
			}
		}

		prod: {
			destinations: [#Terraform & {destination: "infrastructure-prod.*"}]
			config: {
				replicas: 5
				env_vars: RUST_LOG: "warn"
			}
		}
	}

	config: tf.#Spec & {
		name: "service-example"
		ports: [
			{name: "external", port: 3000, external: true},
			{name: "internal", port: 3001},
			{name: "grpc_external", port: 4000, external: true, subdomain: "grpc"},
			{name: "grpc_internal", port: 4001},
		]
		health_checks: live: http: {
			path: "/"
			port: 3001
		}
		env_vars: {
			RUST_LOG: "my_service=debug,info"
		}
	}
}

commands: sdk.#ForestProjectCommands & {
	dev: ["cargo run"]
}
