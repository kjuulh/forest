package component_v2

#ForestComponent: {
	name:    string & =~"^[a-z][a-z0-9-]*$"
	org:     string
	version: string & =~#"^\d\.\d\.\d"#
	codegen: #ForestCodegen
}

#ForestCommands: {
	[string]: #ForestCommand
}

#ForestCommand: {
	description: string
	input: {
		...
	}
	output: {
		...
	}
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

#ForestSource: "rust" | "go"
