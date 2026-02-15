package component_v2

project: #ForestProject & {
	name:         "ecs-service"
	organisation: "rawpotion"
}

forest: component: #ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/ecs-service/src/"
	}

	upload: {
		source: "./crates/ecs-service"
		type:   "rust"
		architectures: {
			linux: {
				amd64: {}
				// arm64: {}
			}
			// macos: {
			// 	amd64: {}
			// 	arm64: {}
			// }
			// windows: {
			// 	amd64: {}
			// 	arm64: {}
			// }
		}
	}

	// Docker alternative — outputs OCI tar files instead of native binaries.
	// Uncomment below (and comment out the upload above) to use docker builds:
	//
	// upload: {
	// 	source: "./crates/ecs-service"
	// 	type:   "docker"
	// 	architectures: {
	// 		linux: {
	// 			amd64: {}
	// 			arm64: {}
	// 		}
	// 	}
	// }
}
