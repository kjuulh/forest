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

// A component is published with `forest publish` and consumed by version:
// projects through `dependencies`, tools through `forest global add`.
//
// Every published version is immutable, prereleases included: publishing the
// same version again is refused, so each new build needs a new version.
#ForestComponent: {
	name: string

	// A SemVer version. A stable release (`1.3.0`) is what consumers get by
	// default. A prerelease (`1.3.0-rc.1`, `1.3.0-alpha.10`) is never picked
	// for anyone who did not ask for it: it is installed by exact version
	// (`@1.3.0-rc.1`) or by opting a tool in with `forest global add
	// <org>/<name> --prerelease`. Number prereleases upwards (`rc.1`, `rc.2`,
	// …, compared numerically) and publish the final `1.3.0` when done; it
	// supersedes its RCs for everyone following them. Set a prerelease
	// without editing this file with `FOREST_COMPONENT_VERSION=1.3.0-rc.1`:
	// `forest publish` publishes under it, and forest's rust/go/docker build
	// components stamp it into the binary. A build of your own (`prebuilt`)
	// must stamp it itself; `forest components version` prints the version
	// `forest publish` will use. Build metadata (`1.3.0+build.5`) is not a
	// prerelease.
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
