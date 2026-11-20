package forest_ripgrep

import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
	name:             "rg"
	argv_passthrough: true
	description:      "Fast recursive grep, by BurntSushi"
}
