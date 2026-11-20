package deployment

import "forest.sh/forest/sdk@v0"

#DeploymentHooks: sdk.#ForestHook & {
	prepare: {
		description: string | *"Prepare deployment artifacts"
		input: {}
		output: {
			manifests: [...#Manifest]
		}
	}
	release: {
		description: string | *"Execute deployment"
		input: {
			release_id: string
		}
		output: {}
	}
	rollback: {
		description: string | *"Roll back deployment"
		input: {
			release_id:      string
			target_revision: string | *""
		}
	}
}

#Manifest: {
	name:    string & =~"^[a-z0-9][a-z0-9._-]*$"
	content: string
}
