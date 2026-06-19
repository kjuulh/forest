package init_component

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "init"
	organisation: "forest-contrib"
	description:  "Render a small project skeleton (currently `rust-cli`) into a work dir. Bootstrap step for fresh repos."
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
		output: "./crates/init/src/"
	}

	upload: {
		source: "./crates/init"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
