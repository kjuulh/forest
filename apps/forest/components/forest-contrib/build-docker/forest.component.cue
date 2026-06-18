package build_docker

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	build: {
		description: "Compile the depending component with docker for its declared platforms."
		input: {}
		output: {...}
	}
}
