package build_docker_example

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-docker-example"
	organisation: "examples"
	description:  "Demonstrates the DATA-312 build path with Docker: depends on forest-contrib/build-docker."
}

dependencies: {
	"forest-contrib/build-docker": path: "../../components/forest-contrib/build-docker"
}

"forest-contrib": "build-docker": {}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"
	upload: {
		source: "."
		type:   "docker"
		architectures: {
			linux: amd64: {}
		}
	}
}
