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
	// header. Forest publish pushes this to the server on every publish;
	// missing here = cleared server-side. See specs/features/009-project-metadata.md.
	description?: string

	// Blessed project metadata. Surfaced in the project Overview's
	// "About" sidebar (links + identity). Field set is intentionally
	// small; new keys require a spec update.
	metadata?: #ProjectMetadata
}

#ProjectMetadata: {
	// Upstream source repository (rendered as a link).
	git_url?: string

	// Public landing page / marketing site (rendered as a link).
	homepage?: string

	// Docs site URL (rendered as a link).
	docs_url?: string

	// Issue tracker / Slack channel / on-call link (rendered as a link).
	support_url?: string

	// Business or team domain — e.g. "payments", "infra".
	domain?: string

	// Responsible team or person (free-form string).
	owner?: string

	// Free-form labels used by organisation-scoped rule selectors.
	tags?: [...string]
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

	// Artifacts shipped alongside the published binary and materialised into
	// the local cache when the tool is fetched. See TASKS/023-global-tool-env.md.
	include?: #ForestInclude

	// External tools this component shells out to at runtime (e.g. a build
	// component that invokes `cargo`). Forest verifies each is on PATH before
	// dispatching to the component and fails up front with an actionable
	// diagnostic, rather than letting the missing binary blow up mid-run.
	// DATA-312.
	requires?: #ForestRequires
}

// `requires` — the component's runtime tool contract. Forest checks these are
// present on PATH before invoking the component.
#ForestRequires: {
	tools?: [...#ForestRequiredTool]
}

#ForestRequiredTool: {
	// Binary expected on PATH, e.g. "cargo", "go", "docker".
	name: string & =~"^[a-zA-Z][a-zA-Z0-9._-]*$"

	// Optional install hint shown when the tool is missing.
	hint?: string
}

// `include` — things shipped beside the binary. Forward-looking container
// (future: files, …). TASKS/023, DATA-588.
#ForestInclude: {
	// Default environment variables, auto-applied as defaults when the tool
	// runs (the ambient shell environment always wins). Keys are POSIX env
	// names; values are plain strings.
	env?: {[=~"^[A-Za-z_][A-Za-z0-9_]*$"]: string}

	// Shell integration this tool ships. Declaring it here is what lets
	// `eval "$(forest shell zsh)"` load the tool's completions/functions with
	// no per-tool line in the user's rc file. DATA-588.
	shell?: #ForestShellIntegration
}

// `include.shell` — how forest obtains this tool's shell-integration script.
//
// Tools that ship an rc-file snippet used to make every user add
// `eval "$(<tool> init zsh)"` by hand — a manual step which, because global
// tools install lazily, also turned shell startup into a cold-cache download.
// Declaring it here inverts that: forest runs the command once when the tool is
// fetched, caches the output, and serves it from `forest shell <shell>`.
//
//	include: shell: init: {
//	    zsh:  ["init", "zsh"]
//	    fish: ["completion", "fish"]
//	}
#ForestShellIntegration: {
	// Shell name → argv to run against this tool's own binary to print its
	// integration script on stdout. Keyed by shell because tools spell it
	// differently (`init zsh` vs `completion fish`) and may not support all
	// three. Omit a shell to opt out of it; the argv must be non-empty.
	init?: {[=~"^(zsh|bash|fish)$"]: [...string] & [_, ...]}
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

