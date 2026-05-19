// The deployment contract defines the lifecycle hooks for deploying
// services to infrastructure targets (Kubernetes, ECS, Terraform, etc.).
//
// Components implementing this contract participate in:
//   - forest release prepare   → hooks/forest/deployment/prepare
//   - forest release           → hooks/forest/deployment/release (server-side)
//   - forest release rollback  → hooks/forest/deployment/rollback (server-side)
package deployment

// DeploymentHooks defines the required hook signatures for the
// forest/deployment contract. Components must implement all three hooks.
#DeploymentHooks: {
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
