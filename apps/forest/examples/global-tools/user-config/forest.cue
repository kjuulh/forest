package forest

import sdk "forest.sh/forest/sdk@v0"

config: sdk.#UserConfig & {
	user: {
		author: "alice@example.com"
	}

	dependencies: {
		"cuteorg/ripgrep": {
			version: "14.1.1"
		}
	}

	org_catalog: {
		cuteorg: {
			enabled: true
			banned:  ["forest-greet"]
			pins: {}
			aliases: {}
		}
	}
}
