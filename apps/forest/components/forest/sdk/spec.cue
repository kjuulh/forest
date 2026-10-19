package sdk

#ForestProject: {
	name:         string & =~"^[a-z][a-z0-9-]*$"
	organisation: string & =~"^[a-z][a-z0-9-]*$"

	description?: string

	metadata?: #ProjectMetadata

	env?: {
		[string]: #ForestEnvironmentConfig
	}
}

#ProjectMetadata: {
	git_url?: string
	homepage?: string
	docs_url?: string
	support_url?: string
	domain?: string
	owner?: string
	tags?: [...string]
}

#ForestComponent: {
	name:    string
	version: string & =~#"^\d+\.\d+\.\d+"#

	codegen?: #ForestCodegen
	upload?:  #ForestComponentUpload

	paths?: #ForestComponentPaths

	external?: #ForestExternal

	include?: #ForestInclude

	requires?: #ForestRequires

	paths?: #ForestPaths
}

#ForestPaths: {
	exclude?: [...string]

	include?: [...string]
}

#ForestRequires: {
	tools?: [...#ForestRequiredTool]
}

#ForestRequiredTool: {
	name: string & =~"^[a-zA-Z][a-zA-Z0-9._-]*$"

	hint?: string
}

#ForestInclude: {
	env?: {[=~"^[A-Za-z_][A-Za-z0-9_]*$"]: string}

	shell?: #ForestShellIntegration
}

#ForestShellIntegration: {
	init?: {[=~"^(zsh|bash|fish)$"]: [...string] & [_, ...]}
}

#ForestComponentPaths: {
	include?: [...string]
}

#ForestComponentUpload: {
	type:     #ForestSource
	source:   string | *"."
	registry: string | *"registry.forage.sh"

	architectures?: {
		[#ForestArchitectures]: #ForestArchitecture
	}

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

#ForestTool: {
	name: string & =~"^[a-zA-Z][a-zA-Z0-9._-]{0,63}$"

	argv_passthrough: bool | *true

	description?: string
}

#ForestExternal: {
	platforms: [...#ForestExternalPlatform]
}

#ForestExternalPlatform: {
	os:   #ForestArchitectures
	arch: #ForestArch

	url: string & =~"^https://"

	sha256: string & =~#"^[0-9a-f]{64}$"#

	archive: "none" | "tar.gz" | "tar.xz" | "tar.zst" | "zip" | *"none"

	binary_in_archive?: string

	archive_sha256?: string & =~#"^[0-9a-f]{64}$"#

	executable_mode?: string | *"0755"
}

#ForestDependency: {
	version: string
} | {
	path: string
}

#ForestDependencies: {
	[string]: #ForestDependency
}

#ForestProjectCommands: {
	[string]: [...string]
}

#ForestDestinationRef: {
	destination: string
	type:        string
}

#ForestEnvironmentConfig: {
	destinations: [...#ForestDestinationRef]
	config?: {...}
}

#ForestComponentUsage: {
	env?: {
		[string]: #ForestEnvironmentConfig
	}
	config?: {...}
}
