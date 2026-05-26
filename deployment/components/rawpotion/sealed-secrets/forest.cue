package sealed_secrets

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "sealed-secrets"
	organisation: "rawpotion"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": version: "0.3.0"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.2"

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
