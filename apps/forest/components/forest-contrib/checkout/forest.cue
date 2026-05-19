package checkout

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "checkout"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/checkout/src/"
	}

	upload: {
		source: "./crates/checkout"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
