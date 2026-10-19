package render_template

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	"render-template": {
		description: "Walk a source directory, interpolate {{var}} placeholders in file contents and path components, write the rendered tree to a destination directory."
		input: {
			src: string

			dest: string

			vars: [string]: string
		}
		output: {
			files_rendered: int
			src:            string
			dest:           string
		}
	}
}
