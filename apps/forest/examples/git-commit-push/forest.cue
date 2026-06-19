package git_commit_push

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "git-commit-push"
	organisation: "forest-contrib"
	description:  "Stage, commit, and push. Idempotent against partially set-up repos — inits, adds origin, creates the branch as needed."
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
		output: "./crates/git-commit-push/src/"
	}

	upload: {
		source: "./crates/git-commit-push"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
