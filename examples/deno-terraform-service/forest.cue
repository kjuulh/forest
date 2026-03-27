package deno_terraform_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "deno-terraform-service"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "typescript"
		output: "./src/"
	}

	upload: {
		source: "./src"
		type:   "deno"
	}
}
