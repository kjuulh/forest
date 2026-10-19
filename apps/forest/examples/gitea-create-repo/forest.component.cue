package gitea_create_repo

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"gitea-create-repo": {
		description: "Create a repository on a Gitea instance via the REST API. Reads the API token from a file path (typically a Forest Secret mounted at /run/secrets/<name>) so it never appears in logs or process listings."
		input: {
			base_url: string

			org: string | *""

			name: string

			description: string | *""

			private: bool | *true

			auto_init: bool | *false

			default_branch: string | *"main"

			token_path: string
		}
		output: {
			id:        int
			clone_url: string
			ssh_url:   string
			html_url:  string
			full_name: string
		}
	}
}
