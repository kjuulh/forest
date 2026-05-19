# Your First Project

A Forest project is a directory containing a `forest.cue` configuration file that defines your service, its component dependencies, and environment-specific settings.

## Scaffold a Project

```bash
forest init
```

You'll be prompted to choose a starter template and project name. This creates the directory structure:

```
my-service/
  forest.cue          # Project configuration
  cue.mod/
    module.cue        # CUE module metadata
```

Alternatively, specify a starter directly:

```bash
forest init my-starter --dest ./my-service
```

## Register the Project

Create the project on the Forest server:

```bash
forest project create \
  --organisation my-org \
  --name my-service
```

Or initialize from the project directory (reads `forest.cue`):

```bash
forest project init
```

## Add Component Dependencies

Components provide the deployment logic. Add one from the registry:

```bash
# Add a component (latest version)
forest add forest-contrib/kubernetes-service

# Add a specific version
forest add forest-contrib/kubernetes-service@0.2.0

# Add a local path dependency (for development)
forest add forest-contrib/kubernetes-service --path ../my-local-component
```

This updates `forest.cue` and creates/updates `forest.lock`.

## Configure Environments

Edit `forest.cue` to define how the component behaves in each environment:

```cue
package my_service

import (
    "forest.sh/forest/sdk@v0"
    k8s "forest.sh/forest-contrib/kubernetes-service@v0:kubernetes_service"
)

project: sdk.#ForestProject & {
    name:         "my-service"
    organisation: "my-org"
}

dependencies: sdk.#ForestDependencies & {
    "forest-contrib/kubernetes-service": version: "0.1"
}

"forest-contrib": "kubernetes-service": sdk.#ForestComponentUsage & {
    // Environment-specific overrides
    env: {
        dev: {
            destinations: [
                {destination: "k8s-dev", type: "forest/kubernetes@1"},
            ]
            config: replicas: 1
        }
        prod: {
            destinations: [
                {destination: "k8s-prod", type: "forest/kubernetes@1"},
            ]
            config: replicas: 3
        }
    }

    // Shared config across all environments
    config: k8s.#Spec & {
        name:  "my-service"
        image: "registry.example.com/my-service"
        ports: [{name: "http", port: 8080, external: true}]
        health_checks: liveness: http: {
            path: "/health"
            port: 8080
        }
    }
}
```

## Set Up Environments and Destinations

Create the environments and destinations on the server:

```bash
# Create environments
forest environment create --organisation my-org --name dev
forest environment create --organisation my-org --name prod

# Create destinations
forest destination create \
  --organisation my-org \
  --name k8s-dev \
  --environment dev \
  --type forest/kubernetes@1

forest destination create \
  --organisation my-org \
  --name k8s-prod \
  --environment prod \
  --type forest/kubernetes@1
```

## Validate

Verify your configuration is correct:

```bash
forest validate
```

This checks:

- Project config matches component schemas
- All required fields are present
- Contract coverage (which deployment hooks are fulfilled)

## Update Dependencies

Keep dependencies up to date:

```bash
# Update all dependencies
forest update

# Update a specific dependency
forest update forest-contrib/kubernetes-service
```

---

**Next:** [Your First Release](first-release.md)
