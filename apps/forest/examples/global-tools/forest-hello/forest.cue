package forest_hello

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forest-hello"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/forest-hello/src/"
	}

	upload: {
		source: "./crates/forest-hello"
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
