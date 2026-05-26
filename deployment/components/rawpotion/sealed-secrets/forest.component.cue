package sealed_secrets

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
#Spec: sdk.#ForestSpec & {
	name:      string & =~"^[a-z][a-z0-9-]*$"
	namespace: string & =~"^[a-z][a-z0-9-]*$"
}

// --- Commands ---
#Commands: sdk.#ForestCommands & {
	seal: {
		description: "Add or update a sealed secret key"
		input: {
			env:   string
			key:   string
			value: string
			// Path to the kubeseal public certificate file (e.g. pub-cert.pem)
			cert:  string
		}
		output: {}
	}
}

// --- Hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Inject sealed secrets manifest"
		release: description:  "No-op"
		rollback: description: "No-op"
	}
}
