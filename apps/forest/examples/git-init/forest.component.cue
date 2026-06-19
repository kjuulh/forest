package git_init

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"git-init": {
		description: "Initialise a fresh git repository at work_dir with a configured author identity, a chosen branch name, and an empty initial commit. Idempotent: if work_dir is already a git repo, leaves it alone and reports the current HEAD."
		input: {
			branch:     string | *"main"
			user_name:  string | *"forest-bot"
			user_email: string | *"forest-bot@local"
			message:    string | *"initial commit"
		}
		output: {
			branch:               string
			initial_commit_sha:   string
			already_initialized:  bool
		}
	}
}
