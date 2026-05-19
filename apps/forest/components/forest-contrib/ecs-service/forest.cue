// =============================================================================
// ecs-service component — published as
// `forest-contrib/ecs-service@<version>`.
//
// Until first publish, consumers reference this component via a relative
// `path:` in their `forest.cue` (gitnow-workspace assumption). Once we've
// validated a release end-to-end, publish with:
//
//   cd apps/forest/components/forest-contrib/ecs-service
//   mise run forest -- components build
//   mise run forest -- components publish
//
// ...then update consumers to drop the `path:` and let forest resolve from
// the registry. Bump `forest.component.version` (below) per release.
// =============================================================================

package ecs_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "ecs-service"
	organisation: "forest-contrib"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": path: "../../forest/deployment"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/ecs-service/src/"
	}

	upload: {
		source: "./crates/ecs-service"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
