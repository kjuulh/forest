# Projects

A project represents a service or application managed by Forest. It's the top-level unit that ties together components, environments, and releases.

## Definition

Every project has a `forest.cue` file at its root:

```cue
package my_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
    name:         "my-service"
    organisation: "my-org"
}
```

The project name must be lowercase alphanumeric with hyphens (`^[a-z][a-z0-9-]*$`).

## Dependencies

Projects declare component dependencies:

```cue
dependencies: sdk.#ForestDependencies & {
    // From the registry (with version spec)
    "forest-contrib/kubernetes-service": version: "0.1"

    // Local path (for development)
    "forest-contrib/terraform-service": path: "../my-local-component"
}
```

Version specs follow semver matching — `"0.1"` resolves to the latest `0.1.x` release.

## Component Usage

For each component dependency, define per-environment configuration:

```cue
"forest-contrib": "kubernetes-service": sdk.#ForestComponentUsage & {
    env: {
        dev: {
            destinations: [{destination: "k8s-dev", type: "forest/kubernetes@1"}]
            config: replicas: 1
        }
        prod: {
            destinations: [{destination: "k8s-prod", type: "forest/kubernetes@1"}]
            config: replicas: 3
        }
    }
    config: k8s.#Spec & {
        name: "my-service"
        // ... shared config
    }
}
```

The `config` at the top level is shared across all environments. Per-environment `config` overrides or extends it.

## Custom Commands

Projects can define commands that are available via `forest run`:

```cue
commands: sdk.#ForestProjectCommands & {
    dev:  ["cargo run"]
    test: ["cargo test"]
    lint: ["cargo clippy"]
}
```

Run them with:

```bash
forest run dev
forest run test
```

## Lock File

`forest.lock` records the resolved dependency graph:

```
# forest.lock — do not edit manually
forest-contrib/kubernetes-service@0.1.2 linux/amd64 sha256:abc123...
forest-contrib/terraform-service@0.1.0 path:../../components/forest-contrib/terraform-service
```

- **Registry binary deps** are "hard-locked": a binary dependency's sha256 for this platform is checked when downloaded, and a mismatch fails instead of being re-pinned. Since v0.3.17, forest also checks the resolved cached binary digest against the project lock before loading commands.
- **Path deps** are "soft-locked": always resolved from disk.

Since **v0.3.17**, a version range in `forest.cue` (`"0.1"`, `"1"`, `"latest"`) resolves to a matching binary version lock when present, otherwise to the newest published stable match (a prerelease only if no stable version matches). Binary selections are locked when resolved; `forest update` moves a binary lock to the newest match. Released v0.3.16 cannot load these ranges at runtime; use exact published dependency versions there.

Lock-first selection only applies when a registry binary lock entry exists. File/template-only components have no version/digest lock entry and their ranges resolve again; choose exact versions for reproducible selection. Binary digest checks do not establish integrity of component files.

## CLI Commands

```bash
forest project create --organisation my-org --name my-service
forest project init              # Init from forest.cue
forest project list --organisation my-org
forest project releases --organisation my-org --project my-service
forest project publish           # Publish project config
```
