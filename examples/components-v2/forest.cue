package kubernetes_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "kubernetes-service"
	organisation: "forest-contrib"
}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/kubernetes-service/src/"
	}

	upload: {
		source: "./crates/kubernetes-service"
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
		}
	}
}
