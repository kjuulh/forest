package forest_greet

import sdk "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {
	default_name: string | *"world"
}

#Tool: sdk.#ForestTool & {
	name:             "greet"
	argv_passthrough: true
	description:      "Print a friendly greeting (callable as a CLI or as a Forest command)"
}

#Commands: sdk.#ForestCommands & {
	greet: {
		description: "Return a greeting as structured JSON"
		input: {
			name?: string
		}
		output: {
			greeting: string
		}
	}
}
