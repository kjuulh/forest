// forest-hello — a TOOL_BINARY example.
//
// Declares ONLY a #Tool facet, no #Commands.
// Registry will classify this as shape=TOOL_BINARY.
// Invocation: argv passthrough only (no _meta/describe-backed methods).

package forest_hello

import sdk "forest.sh/forest/sdk@v0"

// No input spec — pure argv passthrough.
#Spec: sdk.#ForestSpec & {}

// Tool facet — the only invocation surface.
#Tool: sdk.#ForestTool & {
	name:             "hello"
	argv_passthrough: true
	description:      "Print a friendly greeting"
}

// #Commands deliberately omitted. The binary is a plain CLI tool.
// #Hooks also omitted.
