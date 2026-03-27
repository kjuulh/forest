# CUE Configuration

Forest uses [CUE](https://cuelang.org/) for all configuration. CUE is a data constraint language that combines configuration, schema validation, and code generation in one system.

## Why CUE?

- **Type safety** — Catch misconfiguration before deployment
- **Composability** — Import and extend schemas from components
- **Defaults** — Sensible defaults with override capability
- **Constraints** — Express invariants like `replicas >= 1`
- **No templating** — CUE is evaluated, not templated; no string interpolation bugs

## File Layout

A Forest project typically has:

```
my-project/
  forest.cue          # Main project configuration
  cue.mod/
    module.cue        # CUE module metadata and dependencies
  forest.lock         # Resolved dependency versions
```

A Forest component has:

```
my-component/
  forest.cue              # Component metadata (name, version, build config)
  forest.component.cue    # Spec schema, commands, hooks
  spec.cue                # Additional type definitions (optional)
  cue.mod/
    module.cue            # CUE module metadata
```

## Project Configuration (`forest.cue`)

### Project Declaration

```cue
package my_service

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
    name:         "my-service"
    organisation: "my-org"
}
```

### Dependencies

```cue
dependencies: sdk.#ForestDependencies & {
    // Registry dependency with version spec
    "forest-contrib/kubernetes-service": version: "0.1"

    // Local path dependency (for development)
    "my-org/my-component": path: "../my-component"
}
```

### Component Usage

```cue
import k8s "forest.sh/forest-contrib/kubernetes-service@v0:kubernetes_service"

"forest-contrib": "kubernetes-service": sdk.#ForestComponentUsage & {
    // Per-environment configuration
    env: {
        dev: {
            destinations: [
                {destination: "k8s-dev", type: "forest/kubernetes@1"},
            ]
            config: {
                replicas: 1
            }
        }
        prod: {
            destinations: [
                {destination: "k8s-prod", type: "forest/kubernetes@1"},
            ]
            config: {
                replicas: 5
            }
        }
    }

    // Shared configuration (merged with per-env config)
    config: k8s.#Spec & {
        name:  "my-service"
        image: "registry.example.com/my-service"
        ports: [{name: "http", port: 8080, external: true}]
    }
}
```

### Custom Commands

```cue
commands: sdk.#ForestProjectCommands & {
    dev:  ["cargo run"]
    test: ["cargo test"]
}
```

## Component Configuration

### Metadata (`forest.cue`)

```cue
package my_component

import "forest.sh/forest/sdk@v0"

component: sdk.#ForestComponent & {
    name:    "my-component"
    version: "0.1.0"

    upload: {
        type:   "rust"         // "rust" | "go" | "docker"
        source: "./crates/my-component"
        architectures: {
            linux: amd64: {}
            macos: arm64: {}
        }
    }

    codegen: {
        type:   "rust"
        output: "./crates/my-component/src/"
    }
}
```

### Spec Schema (`forest.component.cue`)

```cue
#Spec: sdk.#ForestSpec & {
    name:     string
    replicas: int & >=1 | *1
    // ... your fields
}

#Commands: sdk.#ForestCommands & {
    // ... your commands
}

#Hooks: sdk.#ForestHooks & {
    // ... your hooks
}
```

## CUE Module (`cue.mod/module.cue`)

```cue
module: "forest.sh/my-org/my-service@v0"
language: version: "v0.12.0"
deps: {
    "forest.sh/forest/sdk@v0": v: "0.1.0"
    "forest.sh/forest-contrib/kubernetes-service@v0": v: "0.1.0"
}
```

## Configuration Merging

CUE configuration merges hierarchically:

1. **Component spec defaults** (from `forest.component.cue`)
2. **Project shared config** (from `config:` in component usage)
3. **Per-environment config** (from `env: { dev: { config: ... } }`)

Per-environment values override shared values. CUE's unification ensures type safety at every level.

## Common Patterns

### Destination Type References

Use a private field to avoid repetition:

```cue
_destinationTypes: {
    terraform: "forest/terraform@1"
    k8s:       "forest/kubernetes@1"
}

env: dev: destinations: [
    {destination: "infra-dev", type: _destinationTypes.terraform},
    {destination: "k8s-dev",   type: _destinationTypes.k8s},
]
```

### Environment-Specific Overrides

```cue
env: {
    dev:     config: env_vars: [{key: "RUST_LOG", value: "debug"}]
    staging: config: env_vars: [{key: "RUST_LOG", value: "info"}]
    prod:    config: env_vars: [{key: "RUST_LOG", value: "warn"}]
}
```

### Multiple Components

A project can use multiple components:

```cue
"forest-contrib": {
    "kubernetes-service": sdk.#ForestComponentUsage & { /* ... */ }
    "terraform-service":  sdk.#ForestComponentUsage & { /* ... */ }
}
```
