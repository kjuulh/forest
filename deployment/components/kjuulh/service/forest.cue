package service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "service"
	organisation: "kjuulh"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment":        version: "0.0.1"
	"kjuulh/sealed-secrets":   path:    "../sealed-secrets"
	"kjuulh/forage-postgresql": path:   "../forage-postgresql"
	"kjuulh/forage-nats":      path:    "../forage-nats"
	"kjuulh/forage-s3":        path:    "../forage-s3"
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
