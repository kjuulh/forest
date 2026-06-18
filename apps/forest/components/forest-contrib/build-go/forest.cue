package build_go

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-go"
	organisation: "forest-contrib"
	description:  "Build component: compiles a forest component with go. Depend on this so `forest run build` builds your project. DATA-312."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	upload: {
		source: "./crates/build-go"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
				arm64: {}
			}
			macos: {
				amd64: {}
				arm64: {}
			}
		}
	}
}
