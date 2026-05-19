package render_template

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "render-template"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/render-template/src/"
	}

	upload: {
		source: "./crates/render-template"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
