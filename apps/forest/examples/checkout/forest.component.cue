package checkout

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	checkout: {
		description: "Shallow-clone a git repository into a destination directory."
		input: {
			// Repository URL or path. Accepts anything `git clone` accepts:
			//   https://github.com/owner/repo.git
			//   git@github.com:owner/repo.git
			//   file:///abs/path/repo
			//   /abs/path/bare.git
			repo: string

			// Branch or tag to check out. When unset, clones the remote's
			// default branch (whatever HEAD points at).
			ref?: string

			// Shallow-clone depth. Set to 0 for a full clone.
			depth: int | *1

			// Destination directory. Created by git clone; must NOT already
			// exist as a non-empty directory.
			dest: string
		}
		output: {
			commit_sha: string
			branch:     string
			dest:       string
		}
	}
}
