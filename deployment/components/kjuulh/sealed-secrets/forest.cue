package sealed_secrets

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "sealed-secrets"
	organisation: "kjuulh"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": version: "0.0.1"
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

commands: sdk.#ForestCommands & {}
