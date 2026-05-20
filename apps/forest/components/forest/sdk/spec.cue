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
}

#ForestComponentPaths: {
	include?: [...string]
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

#ForestSource: "rust" | "go" | "docker" | "typescript" | "deno"

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
