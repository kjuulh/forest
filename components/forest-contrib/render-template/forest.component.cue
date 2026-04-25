package render_template

import "forest.sh/forest/sdk@v0"

// --- Input spec: shared across all commands. render-template is a
// stateless action — there's no project-scoped configuration, so the
// spec is intentionally empty. (Components that wrap stateful destinations
// like terraform-service use the spec for configuration; actions don't.)
#Spec: sdk.#ForestSpec & {}

// --- Commands: actions the component can perform ---
#Commands: sdk.#ForestCommands & {
	"render-template": {
		description: "Walk a source directory, interpolate {{var}} placeholders in file contents and path components, write the rendered tree to a destination directory."
		input: {
			// Source directory containing templates. Walked recursively.
			src: string

			// Destination directory. Created if absent. Existing files
			// are overwritten without warning.
			dest: string

			// Interpolation values. Each `{{key}}` (with optional surrounding
			// whitespace) in a file's contents OR path is replaced with
			// `vars[key]`. Unknown placeholders abort with an error.
			vars: [string]: string
		}
		output: {
			files_rendered: int
			src:            string
			dest:           string
		}
	}
}
