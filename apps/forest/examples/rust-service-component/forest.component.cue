package rust_service

#ForestProject: {
	name:         string & =~"^[a-z][a-z0-9-]*$"
	organisation: string & =~"^[a-z][a-z0-9-]*$"
}

#ForestComponent: {
	name:    string
	version: string & =~#"^\d\.\d\.\d"#

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

#ForestSpec: {...}

#ForestHooks: {
	[string]: #ForestHook
}

#ForestHook: {...}

#ForestCodegen: {
	type:   #ForestSource
	output: string
}

#ForestSource: "rust" | "go" | "docker"
