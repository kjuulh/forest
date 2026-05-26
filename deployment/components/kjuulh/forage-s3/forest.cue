package forage_s3

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forage-s3"
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
