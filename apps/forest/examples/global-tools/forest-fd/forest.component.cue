package forest_fd

import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
	name:             "fd"
	argv_passthrough: true
	description:      "Simple, fast and user-friendly alternative to 'find'"
}
