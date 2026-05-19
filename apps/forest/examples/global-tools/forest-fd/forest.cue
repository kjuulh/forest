package forest_fd

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "fd"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "10.2.0"

	external: sdk.#ForestExternal & {
		platforms: [
			{
				os:                "linux"
				arch:              "amd64"
				url:               "https://github.com/sharkdp/fd/releases/download/v10.2.0/fd-v10.2.0-x86_64-unknown-linux-musl.tar.gz"
				archive:           "tar.gz"
				binary_in_archive: "fd-v10.2.0-x86_64-unknown-linux-musl/fd"
				sha256:            "9b2c1e7f8a4d3e2c1b9a8e7d6c5b4a3928f7e6d5c4b3a2918e7f6d5c4b3a2918"
				archive_sha256:    "7e6d5c4b3a2918e7f6d5c4b3a2918e7f6d5c4b3a2918e7f6d5c4b3a2918e7f6d"
			},
			{
				os:                "macos"
				arch:              "arm64"
				url:               "https://github.com/sharkdp/fd/releases/download/v10.2.0/fd-v10.2.0-aarch64-apple-darwin.tar.gz"
				archive:           "tar.gz"
				binary_in_archive: "fd-v10.2.0-aarch64-apple-darwin/fd"
				sha256:            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
				archive_sha256:    "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
			},
		]
	}
}
