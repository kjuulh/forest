package forest_ripgrep

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "ripgrep"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "14.1.1"

	// No `codegen`, no `upload`. The `external` block makes this a
	// kind=external manifest at publish time — `forest components publish`
	// dispatches on the presence of `external:` vs `upload:`.
	external: sdk.#ForestExternal & {
		platforms: [
			{
				os:                "linux"
				arch:              "amd64"
				url:               "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
				archive:           "tar.gz"
				binary_in_archive: "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg"
				// Run `forest tool hash <url> --archive tar.gz --binary-in-archive ...`
				// to compute these. Placeholders below are illustrative.
				sha256:         "ad3a44e3d8b8a9d39c1f7b4d1a9b9e3a5e7c2f6c8b4f3a1d2e9c8b7a6e5d4c3b"
				archive_sha256: "4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef09337b3995f5f"
			},
			{
				os:                "macos"
				arch:              "arm64"
				url:               "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-aarch64-apple-darwin.tar.gz"
				archive:           "tar.gz"
				binary_in_archive: "ripgrep-14.1.1-aarch64-apple-darwin/rg"
				sha256:            "c4b5e3a1f9d2e8b7a6c5d4b3a2918e7f6d5c4b3a2918e7f6d5c4b3a2918e7f6d"
				archive_sha256:    "fa9c8b7a6e5d4c3b2918e7f6d5c4b3a2918e7f6d5c4b3a2918e7f6d5c4b3a291"
			},
		]
	}
}
