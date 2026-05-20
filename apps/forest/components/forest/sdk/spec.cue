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

	// Optional human-readable description shown on the project Overview
	// header. `forest publish` pushes this to the server on every
	// publish; missing here = cleared server-side.
	// See specs/features/009-project-metadata.md.
	description?: string

	// Blessed project metadata. Surfaced in the project Overview's
	// "About" sidebar (links + identity).
	metadata?: #ProjectMetadata
}

#ProjectMetadata: {
	// Upstream source repository.
	git_url?: string
	// Public landing page / marketing site.
	homepage?: string
	// Docs site URL.
	docs_url?: string
	// Issue tracker / Slack channel / on-call link.
	support_url?: string
	// Business or team domain — e.g. "payments", "infra".
	domain?: string
	// Responsible team or person.
	owner?: string
}

#ForestComponent: {
	name:    string
	version: string & =~#"^\d+\.\d+\.\d+"#

	codegen?: #ForestCodegen
	upload?:  #ForestComponentUpload

	// Optional file-set declaration. When `paths.include` is set,
	// `forest components publish` only uploads files matching one of
	// the globs (built-in safety excludes still apply on top). Absent
	// ⇒ "include everything except defaults and `.forestignore`".
	paths?: #ForestComponentPaths

	// Alternative to `upload`: declare an external manifest pointing at
	// upstream URLs (e.g. GitHub Releases). Mutually exclusive with
	// `upload` at publish time. See TASKS/018-global-tools.md §1a.2b.
	external?: #ForestExternal
}

#ForestComponentPaths: {
	include?: [...string]
}

#ForestComponentUpload: {
	type:     #ForestSource
	source:   string | *"."
	registry: string | *"registry.forage.sh"

	// For type ∈ {rust, go, docker, typescript, deno}: the cross-compile
	// matrix `forest build` should produce. For type=prebuilt this is
	// optional — the platform set is derived from `prebuilt` instead.
	architectures?: {
		[#ForestArchitectures]: #ForestArchitecture
	}

	// type=prebuilt: per-platform paths (relative to forest.cue) of
	// existing binaries to upload as-is. Skips `forest build` and the
	// `_meta/describe` probe — the tool facet must be declared via
	// `#Tool` in forest.component.cue. The uploaded payload still
	// lands as `kind=binary`, so downloads are auth-gated through the
	// registry exactly like a built component.
	prebuilt?: {
		[#ForestArchitectures]: {
			[#ForestArch]: string
		}
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

#ForestSource: "rust" | "go" | "docker" | "typescript" | "deno" | "prebuilt"

// ============================================================
// Tools (TASKS/018-global-tools.md §1a.1)
// ============================================================

// Tool facet attached to a component. Presence in `forest.component.cue`
// makes the component callable as a CLI tool via argv passthrough.
// A `#Tool` may coexist with `#Commands` (HYBRID_COMPONENT) or stand alone
// (TOOL_BINARY when paired with `upload:`, TOOL_EXTERNAL when paired with
// `external:`).
#ForestTool: {
	// Shim filename on PATH. Must match the regex below.
	name: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"

	// In-scope value: true. `false` is reserved for a future spec.
	argv_passthrough: bool | *true

	// Optional one-line description rendered by `forest global list` /
	// `forest components search`.
	description?: string
}

// ============================================================
// External tools (TASKS/018-global-tools.md §1a.2b)
// ============================================================

// External manifest: the component is hosted outside the Forest
// registry (typically GitHub Releases). `forest publish` does not
// build a binary — it just records the upstream URLs + hashes.
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

// ============================================================
// Consumer project types
// ============================================================

// Dependency declaration — either a version spec or a local path.
#ForestDependency: {
	version: string
} | {
	path: string
}

// Dependencies map: component reference → dependency spec.
// Keys are "org/name" format (e.g., "forest-contrib/terraform-service").
#ForestDependencies: {
	[string]: #ForestDependency
}

// Project-level commands (e.g., dev, build, test).
// Each command is a list of shell commands executed sequentially.
#ForestProjectCommands: {
	[string]: [...string]
}

// Destination reference inside an environment config.
#ForestDestinationRef: {
	destination: string
	type:        string
}

// Per-environment configuration for a component.
#ForestEnvironmentConfig: {
	destinations: [...#ForestDestinationRef]
	config?: {...}
}

// Component usage block in a consumer project.
// Maps environment names to their config + a base config.
#ForestComponentUsage: {
	env?: {
		[string]: #ForestEnvironmentConfig
	}
	config?: {...}
}
