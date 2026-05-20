package deployment

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "deployment"
	organisation: "forest"
	description:  "Deployment hook contract — the trait a component implements to plug into Forest's release pipeline (prepare → plan → deploy → status)."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

forest: component: {
	name:    "deployment"
	version: "0.3.0"
}
