package forest_hello

import sdk "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Tool: sdk.#ForestTool & {
	name:             "hello"
	argv_passthrough: true
	description:      "Print a friendly greeting"
}
