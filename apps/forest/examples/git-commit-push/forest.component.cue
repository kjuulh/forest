package git_commit_push

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"git-commit-push": {
		description: "Stage, commit, and push the contents of a working directory to a remote URL. If the working directory is not already a git repo it is initialised; if the remote is not yet wired up it is added as `origin`."
		input: {
			repo: string

			remote_url: string

			branch: string | *"main"

			message: string

			user_name:  string | *"forest-bot"
			user_email: string | *"forest-bot@local"

			allow_empty: bool | *false
		}
		output: {
			commit_sha:     string
			pushed_branch:  string
			remote_url:     string
		}
	}
}
