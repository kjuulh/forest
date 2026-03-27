package terraform_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "terraform-service"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

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
