package forest_greet

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forest-greet"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/forest-greet/src/"
	}

	upload: {
		source: "./crates/forest-greet"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
			macos: {
				arm64: {}
			}
		}
	}
}
