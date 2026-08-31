package build_go

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-go"
	organisation: "forest-contrib"
	description:  "Build component: compiles a forest component with go. Depend on this so `forest run build` builds your project. DATA-312."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

forest: component: sdk.#ForestComponent & {
	name: project.name
	// 0.1.2 and 0.1.3 were published without this file being bumped — the
	// registry ran ahead of the repo because build components were released by
	// hand from a laptop. 0.1.4 resyncs it and is released by
	// `.github/workflows/publish-build-component.yml` (DATA-583).
	//
	// This is the first build-go carrying the DATA-583 forest-build-core:
	// `FOREST_COMPONENT_VERSION` overrides the manifest version so a
	// prerelease CI tag stamps the version it publishes under, and
	// `FOREST_BUILD_TARGETS` narrows the build so a matrix can split platforms
	// across native runners. It also carries the fix for the `go -ldflags …
	// build` argument order, which would have broken every Go build the moment
	// this component was next published.
	// Republished for the cue.mod fix (#212/#215): every version below this
	// one was published without cue.mod/module.cue, so its `import
	// "forest.sh/forest/sdk@v0"` cannot resolve and the component fails to
	// parse wherever it is used. Inert for existing consumers — dependents
	// pin an exact version, so nothing picks this up until a repo bumps.
	version: "0.1.5"

	upload: {
		source: "./crates/build-go"
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
