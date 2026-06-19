package checkout

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "checkout"
	organisation: "forest-contrib"
	description:  "Forest-shaped `git clone` — spiritual successor to GitHub's actions/checkout. Works against any URL git understands."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
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
