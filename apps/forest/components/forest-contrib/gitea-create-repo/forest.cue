package gitea_create_repo

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "gitea-create-repo"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/gitea-create-repo/src/"
	}

	upload: {
		source: "./crates/gitea-create-repo"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
