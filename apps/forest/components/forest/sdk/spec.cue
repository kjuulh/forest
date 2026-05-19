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
