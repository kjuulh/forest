package forage_nats

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
#Spec: sdk.#ForestSpec & {
	name:      string & =~"^[a-z][a-z0-9-]*$"
	namespace: string & =~"^[a-z][a-z0-9-]*$"
	account:   string
	publish: [...{subject: string}]
	subscribe: [...{subject: string}]
	secret_name?: string
}

// --- Commands ---
#Commands: sdk.#ForestCommands & {}

// --- Hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Inject NatsUser CR manifest"
		release: description:  "No-op"
		rollback: description: "No-op"
	}
}
