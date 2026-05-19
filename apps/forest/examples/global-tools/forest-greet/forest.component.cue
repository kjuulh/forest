// forest-greet — a HYBRID_COMPONENT example.
//
// Declares BOTH a #Tool facet AND a #Commands map. Registry will classify
// this as shape=HYBRID_COMPONENT. Same binary, two doorways:
//   - `forest run greet --name=world`           (component protocol)
//   - `greet world`  (via shim, argv passthrough)

package forest_greet

import sdk "forest.sh/forest/sdk@v0"

// Optional input spec — only used by the component-protocol invocation path.
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
