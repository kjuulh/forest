package build_go

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {}

#Commands: sdk.#ForestCommands & {
	build: {
		description: "Compile the depending component with go for its declared platforms."
		input: {}
		output: {...}
	}
}
