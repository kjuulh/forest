package gitea_create_repo

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"gitea-create-repo": {
		description: "Create a repository on a Gitea instance via the REST API. Reads the API token from a file path (typically a Forest Secret mounted at /run/secrets/<name>) so it never appears in logs or process listings."
		input: {
			// Gitea instance base URL, e.g. "https://gitea.example.com".
			base_url: string

			// Owner of the new repo. When set, posts to
			// /api/v1/orgs/{org}/repos. When empty, posts to
			// /api/v1/user/repos (creates under the authenticated user).
			org: string | *""

			// Repository name (must match Gitea's slug rules).
			name: string

			// Optional human description shown on the Gitea UI.
			description: string | *""

			// Visibility. Mirrors Gitea's `private` field.
			private: bool | *true

			// Auto-init flag. When true Gitea creates an initial commit
			// with an empty README so the repo is immediately clonable.
			auto_init: bool | *false

			// Default branch name for the new repo. Gitea defaults to
			// "main" when unset, but we set it explicitly for
			// determinism.
			default_branch: string | *"main"

			// Path to a file containing the Gitea API token. Read at
			// invocation time; never echoed. Use the runner's secret
			// channel to deliver this file inside the VM.
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
