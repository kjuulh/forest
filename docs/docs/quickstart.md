# Quickstart

Get a Forest project running in under 5 minutes.

## Prerequisites

- [Rust toolchain](https://rustup.rs/) (for building from source)
- Access to a Forest server (self-hosted or managed)

## Install Forest

```bash
cargo install --path crates/forest
```

## Authenticate

```bash
forest auth login
```

Follow the prompts to authenticate with your Forest server.

## Create a Project

```bash
# Scaffold from a starter template
forest init
```

This creates a `forest.cue` configuration file and sets up the project structure.

## Add a Component

```bash
# Add a Kubernetes deployment component
forest add forest-contrib/kubernetes-service
```

This adds the component as a dependency in your `forest.cue` and updates `forest.lock`.

## Configure Your Service

Edit `forest.cue` to configure the component for your environments:

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
    env: {
        dev: {
            destinations: [
                {destination: "k8s-dev", type: "forest/kubernetes@1"},
            ]
            config: replicas: 2
        }
        prod: {
            destinations: [
                {destination: "k8s-prod", type: "forest/kubernetes@1"},
            ]
            config: replicas: 5
        }
    }
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

## Validate

```bash
forest validate
```

Checks your configuration against component schemas and verifies contract coverage.

## Prepare and Release

```bash
# Generate deployment manifests
forest release prepare

# Create and execute a release in one step
forest release create --environment dev
```

Forest will prepare manifests, create a release annotation, and deploy to the `dev` environment.

## Watch the Release

The `release create` command streams progress by default. You can also watch any release with:

```bash
forest release wait <release-intent-id>
```

---

**Next steps:** Read the [Getting Started guide](getting-started/index.md) for a deeper walkthrough, or explore [Concepts](concepts/index.md) to understand the full model.
