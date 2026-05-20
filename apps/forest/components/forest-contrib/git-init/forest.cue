package git_init

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "git-init"
	organisation: "forest-contrib"
	description:  "Initialise a fresh git repository with a configured identity and an empty initial commit. Idempotent."
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
