// Forest SDK base types for component definitions.
//
// These types are the contract between component authors and the Forest runtime.
// Import this file alongside your forest.component.cue to get the base types.
//
// Usage:
//   #Spec: #ForestSpec & { ... }
//   #Commands: #ForestCommands & { ... }
//   #Hooks: #ForestHooks & { ... }

package sdk

#ForestProject: {
	name:         string & =~"^[a-z][a-z0-9-]*$"
	organisation: string & =~"^[a-z][a-z0-9-]*$"
}

#ForestComponent: {
	name:    string
	version: string & =~#"^\d+\.\d+\.\d+"#

	codegen?: #ForestCodegen
	upload?:  #ForestComponentUpload

	// Alternative to `upload`: declare an external manifest pointing at
	// upstream URLs. Mutually exclusive with `upload` at publish time
	// (see TASKS/018-global-tools.md §1a.2b).
	external?: #ForestExternal
}

#ForestComponentUpload: {
	type:     #ForestSource
	source:   string | *"."
	registry: string | *"registry.forage.sh"
	architectures: {
		[#ForestArchitectures]: #ForestArchitecture
	}
}

#ForestArchitectures: "linux" | "macos" | "windows"
#ForestArch:          "amd64" | "arm64"

#ForestArchitecture: {
	[#ForestArch]: {}
}

#ForestCommands: {
	[string]: #ForestCommand
}

#ForestCommand: {
	description: string
	input: {...}
	output: {...}
}

#ForestSpec: {
	...
}

#ForestHooks: {
	[string]: #ForestHook
}

#ForestHook: {
	...
}

#ForestCodegen: {
	type:   #ForestSource
	output: string
}

#ForestSource: "rust" | "go" | "docker"

// --- Tools (TASKS/018-global-tools.md §1a.1) ---

// Tool facet attached to a component. Presence in `forest.component.cue`
// makes the component callable as a CLI tool via argv passthrough.
// A `#Tool` may coexist with `#Commands` (HYBRID_COMPONENT) or stand alone
// (TOOL_BINARY when paired with `upload:`, TOOL_EXTERNAL when paired with `external:`).
#ForestTool: {
	// Shim filename on PATH. Must match the regex below.
	name: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"

	// In-scope value: true. `false` is reserved for a future spec.
	argv_passthrough: bool | *true

	// Optional one-line description rendered by `forest global list` / search.
	description?: string
}

// --- External tools (TASKS/018-global-tools.md §1a.2b) ---

#ForestExternal: {
	platforms: [...#ForestExternalPlatform]
}

#ForestExternalPlatform: {
	os:   #ForestArchitectures
	arch: #ForestArch

	// HTTPS-only. `http://` and `file://` are rejected at publish time.
	url: string & =~"^https://"

	// Extracted-binary sha256 (the bytes that get exec'd).
	sha256: string & =~#"^[0-9a-f]{64}$"#

	// Archive format. `none` means the URL serves a bare executable.
	archive: "none" | "tar.gz" | "tar.xz" | "tar.zst" | "zip" | *"none"

	// Path within the archive to the binary. Required iff archive != "none".
	// Must canonicalise per TASKS/018-global-tools.md §1a.2d.
	binary_in_archive?: string

	// Optional sha256 of the downloaded archive (defence-in-depth).
	archive_sha256?: string & =~#"^[0-9a-f]{64}$"#

	// Posix mode applied after extraction. Default 0755.
	executable_mode?: string | *"0755"
}

