package git_commit_push

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"git-commit-push": {
		description: "Stage, commit, and push the contents of a working directory to a remote URL. If the working directory is not already a git repo it is initialised; if the remote is not yet wired up it is added as `origin`."
		input: {
			// Working directory to commit. Must be on a local filesystem.
			repo: string

			// Remote URL or path. Anything `git push` accepts. The remote
			// is added as `origin` if not already present.
			remote_url: string

			// Branch to push. Created if it doesn't exist locally.
			branch: string | *"main"

			// Commit message.
			message: string

			user_name:  string | *"forest-bot"
			user_email: string | *"forest-bot@local"

			// When true, allow an empty commit even if there's nothing
			// staged (mostly useful for sentinel commits).
			allow_empty: bool | *false
		}
		output: {
			commit_sha:     string
			pushed_branch:  string
			remote_url:     string
		}
	}
}
