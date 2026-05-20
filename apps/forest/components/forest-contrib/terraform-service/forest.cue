package terraform_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "terraform-service"
	organisation: "forest-contrib"
	description:  "Deploy a Terraform-managed service. Implements the forest/deployment hooks for terraform-shaped destinations."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": path: "../../forest/deployment"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.2.0"

	codegen: {
		type:   "rust"
		output: "./crates/terraform-service/src/"
	}

	upload: {
		source: "./crates/terraform-service"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
