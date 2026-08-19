package forest_hello

import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forest-hello"
	organisation: "cuteorg"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/forest-hello/src/"
	}

	// Component-declared shell integration (DATA-588). Forest runs
	// `forest-hello shell <shell>` once when the tool is fetched, caches the
	// output, and serves it from `forest shell <shell>` — so a user who runs
	// `forest global add cuteorg/forest-hello` gets the `hello-forest` function
	// in new shells without touching their rc file.
	include: shell: init: {
		zsh: ["shell", "zsh"]
		bash: ["shell", "bash"]
		fish: ["shell", "fish"]
	}

	upload: {
		source: "./crates/forest-hello"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
			macos: {
				arm64: {}
			}
		}
	}
}
