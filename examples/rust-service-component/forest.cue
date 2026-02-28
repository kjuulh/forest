package rust_service

project: #ForestProject & {
	name:         "rust-service"
	organisation: "forest-contrib"
}

forest: component: #ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/rust-service/src/"
	}

	upload: {
		source: "./crates/rust-service"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
			}
		}
	}
}
