// Sample ~/.config/forest/forest.cue
//
// This is what `forest global add` produces in this demo after Parts 4a and 4b
// of the README. Both subscription modes are present:
//   - `dependencies` for a per-tool pin (cuteorg/ripgrep)
//   - `org_catalog` for a whole-catalogue subscription with a ban list
//
// The file is human-readable and human-editable. Forest writes it
// deterministically (stable key order) so manual + automated edits compose.

package forest

import sdk "forest.sh/forest/sdk@v0"

config: sdk.#UserConfig & {
	user: {
		// arbitrary kv set by `forest global set`
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
