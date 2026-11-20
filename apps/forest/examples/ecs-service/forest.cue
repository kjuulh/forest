package ecs_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "ecs-service"
	organisation: "forest-contrib"
	description:  "Deploy an AWS ECS Fargate service. Implements the forest/deployment hooks for ECS-shaped destinations."
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
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/ecs-service/src/"
	}

	upload: {
		source: "./crates/ecs-service"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
