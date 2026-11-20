package checkout

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	checkout: {
		description: "Shallow-clone a git repository into a destination directory."
		input: {
			repo: string

			ref?: string

			depth: int | *1

			dest: string
		}
		output: {
			commit_sha: string
			branch:     string
			dest:       string
		}
	}
}
