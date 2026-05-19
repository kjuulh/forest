package init_component

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	init: {
		description: "Render a project skeleton (Cargo.toml, src/main.rs, README, .gitignore, forest.cue) into the workspace."
		input: {
			project_name: string
			organisation: string
			template:     string | *"rust-cli"
			license:      string | *"MIT"
		}
		output: {
			files_written: [...string]
			template:      string
			work_dir:      string
		}
	}
}
