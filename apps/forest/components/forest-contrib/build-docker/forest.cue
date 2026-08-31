package build_docker

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-docker"
	organisation: "forest-contrib"
	description:  "Build component: compiles a forest component with docker. Depend on this so `forest run build` builds your project. DATA-312."
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
	// parse wherever it is used.
	//
	// The first attempt at that republish is among them. It was published by
	// forest v0.3.7, whose tag turned out to point at a commit predating the
	// fix, so it shipped the old collect_cue_files and uploaded two CUE files
	// where three were needed. Hence this second bump, published by >= 0.3.8.
	//
	// Inert for existing consumers — dependents pin an exact version, so
	// nothing picks this up until a repo bumps.
	version: "0.1.2"

	upload: {
		source: "./crates/build-docker"
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
