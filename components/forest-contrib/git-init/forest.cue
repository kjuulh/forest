package git_init

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "git-init"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/git-init/src/"
	}

	upload: {
		source: "./crates/git-init"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
