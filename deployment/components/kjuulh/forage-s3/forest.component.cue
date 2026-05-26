package forage_s3

import (
	"forest.sh/forest/sdk@v0"
	"forest.sh/forest/deployment@v0"
)

// --- Input spec ---
#Spec: sdk.#ForestSpec & {
	name:        string & =~"^[a-z][a-z0-9-]*$"
	namespace:   string & =~"^[a-z][a-z0-9-]*$"
	bucket_name: string
	key_name?:   string
	secret_name?: string
	quotas?: {
		max_size?:    int
		max_objects?: int
	}
	permissions?: {
		read:  bool | *true
		write: bool | *true
		owner: bool | *false
	}
}

// --- Commands ---
#Commands: sdk.#ForestCommands & {}

// --- Hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": deployment.#DeploymentHooks & {
		prepare: description:  "Inject S3Bucket CR manifest"
		release: description:  "No-op"
		rollback: description: "No-op"
	}
}
