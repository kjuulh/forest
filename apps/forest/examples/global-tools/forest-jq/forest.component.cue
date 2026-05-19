// forest-jq — simplest TOOL_EXTERNAL case: bare executable, no archive,
// shim name equals tool name.

package forest_jq

import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
	name:             "jq"
	argv_passthrough: true
	description:      "Command-line JSON processor"
}
