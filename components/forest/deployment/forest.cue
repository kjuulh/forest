package deployment

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "deployment"
	organisation: "forest"
}

forest: component: {
	name:    "deployment"
	version: "0.3.0"
}
