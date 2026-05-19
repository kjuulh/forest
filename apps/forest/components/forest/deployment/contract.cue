// Forest Deployment Contract
//
// This contract defines the lifecycle hooks for deployments.
// Components that implement this contract promise to handle
// prepare, release, and rollback lifecycle events.
//
// Usage in a component's forest.component.cue:
//
//   import "forest.sh/forest/deployment@v0"
//
//   #Hooks: sdk.#ForestHooks & {
//       "forest/deployment": deployment.#DeploymentHooks
//   }

package deployment

import "forest.sh/forest/sdk@v0"

// #DeploymentHooks defines the hook signatures for the deployment contract.
// Components implementing this contract must provide all three hooks.
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

// #Manifest is a named output file produced by a deployment hook.
#Manifest: {
	name:    string & =~"^[a-z0-9][a-z0-9._-]*$"
	content: string
}
