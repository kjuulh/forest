package forage_postgresql

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
#Spec: sdk.#ForestSpec & {
	name:             string & =~"^[a-z][a-z0-9-]*$"
	namespace:        string & =~"^[a-z][a-z0-9-]*$"
	database_name:    string & =~"^[a-z_][a-z0-9_]*$"
	secret_name:      string & =~"^[a-z][a-z0-9-]*$"
	secret_namespace?: string & =~"^[a-z][a-z0-9-]*$"
}

// --- Commands ---
#Commands: sdk.#ForestCommands & {}

// --- Hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Inject ForagePostgresql CR manifest"
		release: description:  "No-op"
		rollback: description: "No-op"
	}
}
