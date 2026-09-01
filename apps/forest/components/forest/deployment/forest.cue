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
	name: "deployment"
	// 0.3.1 published without a CUE module, for the reason recorded in the sdk
	// component: the OCI packager rejected the duplicate cue.mod/module.cue and
	// the failure was non-fatal (fixed in #225). Versions are immutable.
	version: "0.3.2"
}
