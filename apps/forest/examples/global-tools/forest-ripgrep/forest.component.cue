// forest-ripgrep — a TOOL_EXTERNAL example as a proper Forest project.
//
// Mirrors the `forest-hello` shape: a `forest.cue` declaring the project +
// component metadata (including the external platform manifest), and this
// `forest.component.cue` declaring the tool facet.
//
// Shape: TOOL_EXTERNAL — no upload, no codegen, no #Commands, no #Hooks,
// no #Spec. The binary lives upstream at the URLs declared in forest.cue.

package forest_ripgrep

import sdk "forest.sh/forest/sdk@v0"

#Tool: sdk.#ForestTool & {
	name:             "rg"
	argv_passthrough: true
	description:      "Fast recursive grep, by BurntSushi"
}

// #Commands, #Hooks, #Spec deliberately omitted — external manifests have
// no describe protocol, so they cannot carry commands or hooks. The
// publish-time validator (rule 2 in §1a.2 of the spec) enforces this.
