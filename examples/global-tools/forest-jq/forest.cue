package forest_jq

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "jq"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "1.7.1"

	external: sdk.#ForestExternal & {
		platforms: [
			{
				os:      "linux"
				arch:    "amd64"
				url:     "https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-amd64"
				archive: "none"
				// Plain executable, no `binary_in_archive`, no `archive_sha256`.
				sha256: "5942c9b0934e510ee61eb3e30273f1b3fe2590df93933a93d7c58b81d19c8ff5"
			},
			{
				os:      "macos"
				arch:    "arm64"
				url:     "https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-macos-arm64"
				archive: "none"
				sha256:  "0bbe619e663e0de2c550be2535d9d0d3ef7c1b75ecb6d28f4e9efb8e1c4f2a3b"
			},
		]
	}
}
