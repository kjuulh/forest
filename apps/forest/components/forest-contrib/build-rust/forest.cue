package build_rust

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-rust"
	organisation: "forest-contrib"
	description:  "Build component: compiles a forest component with cargo. Depend on this so `forest run build` builds your project. DATA-312."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

forest: component: sdk.#ForestComponent & {
	name: project.name
	// Republished for the cue.mod fix (#212/#215): every version below this
	// one was published without cue.mod/module.cue, so its `import
	// "forest.sh/forest/sdk@v0"` cannot resolve and the component fails to
	// parse wherever it is used. Inert for existing consumers — dependents
	// pin an exact version, so nothing picks this up until a repo bumps.
	version: "0.1.1"

	upload: {
		source: "./crates/build-rust"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
				arm64: {}
			}
			macos: {
				amd64: {}
				arm64: {}
			}
		}
	}
}
