package build_go_example

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-go-example"
	organisation: "examples"
	description:  "Demonstrates the DATA-312 build path with Go: depends on forest-contrib/build-go."
}

dependencies: {
	"forest-contrib/build-go": path: "../../components/forest-contrib/build-go"
}

"forest-contrib": "build-go": {}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"
	upload: {
		source: "."
		type:   "go"
		architectures: {
			linux: amd64: {}
			macos: arm64: {}
		}
	}
}
