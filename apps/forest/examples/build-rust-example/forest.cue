package build_rust_example

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-rust-example"
	organisation: "examples"
}

dependencies: {
	"forest-contrib/build-rust": path: "../../components/forest-contrib/build-rust"
}

"forest-contrib": "build-rust": {}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"
	upload: {
		source: "./crates/build-rust-example"
		type:   "rust"
		architectures: {
			macos: arm64: {}
		}
	}
}
