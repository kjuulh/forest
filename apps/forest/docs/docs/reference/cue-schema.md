# CUE Schema Reference

Complete reference for Forest's SDK CUE types. These are defined in `forest.sh/forest/sdk@v0`.

## Project Types

### `#ForestProject`

Top-level project declaration.

```cue
#ForestProject: {
    name:         string & =~"^[a-z][a-z0-9-]*$"
    organisation: string & =~"^[a-z][a-z0-9-]*$"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Project name (lowercase alphanumeric + hyphens) |
| `organisation` | `string` | Organisation name (lowercase alphanumeric + hyphens) |

### `#ForestDependencies`

Dependency declarations. Each key is `"org/name"`:

```cue
dependencies: sdk.#ForestDependencies & {
    "org/component": version: "0.1"     // Registry dependency
    "org/component": path: "../local"   // Path dependency
}
```

### `#ForestComponentUsage`

Per-component configuration in a project:

```cue
#ForestComponentUsage: {
    env: {
        [string]: {
            destinations: [...{
                destination: string
                type:        string
            }]
            config: {...}
        }
    }
    config: {...}  // Shared across environments
}
```

| Field | Description |
|-------|-------------|
| `env` | Per-environment configuration map |
| `env.<name>.destinations` | Destination mappings for this environment |
| `env.<name>.config` | Environment-specific config (overrides shared) |
| `config` | Shared config across all environments |

### `#ForestProjectCommands`

Custom project commands:

```cue
#ForestProjectCommands: {
    [string]: [...string]  // command name → list of shell commands
}
```

---

## Component Types

### `#ForestComponent`

Component metadata declaration.

```cue
#ForestComponent: {
    name:    string
    version: string & =~#"^\d+\.\d+\.\d+"#

    codegen?: #ForestCodegen
    upload?:  #ForestComponentUpload
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | Yes | Component name |
| `version` | `string` | Yes | Semver version (e.g., `"0.1.0"`) |
| `codegen` | `#ForestCodegen` | No | Code generation settings |
| `upload` | `#ForestComponentUpload` | No | Build and upload settings |

### `#ForestComponentUpload`

Build and upload configuration.

```cue
#ForestComponentUpload: {
    type:     #ForestSource           // "rust" | "go" | "docker"
    source:   string | *"."           // Source directory
    registry: string | *"registry.forage.sh"
    architectures: {
        [#ForestArchitectures]: #ForestArchitecture
    }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `#ForestSource` | | Build type: `"rust"`, `"go"`, or `"docker"` |
| `source` | `string` | `"."` | Source directory relative to component root |
| `registry` | `string` | `"registry.forage.sh"` | Target registry |
| `architectures` | map | | Target platforms |

### `#ForestArchitectures`

Supported operating systems: `"linux"` | `"macos"` | `"windows"`

### `#ForestArch`

Supported CPU architectures: `"amd64"` | `"arm64"`

### `#ForestCodegen`

Code generation configuration.

```cue
#ForestCodegen: {
    type:   #ForestSource   // "rust" | "go" | "docker"
    output: string           // Output directory
}
```

### `#ForestSource`

Build/codegen source type: `"rust"` | `"go"` | `"docker"`

---

## Spec and Command Types

### `#ForestSpec`

Base type for component specs. Open struct — components extend it:

```cue
#ForestSpec: {
    ...  // Open: any fields allowed
}

// Usage:
#Spec: sdk.#ForestSpec & {
    name:     string
    replicas: int | *1
    // Your fields here
}
```

### `#ForestCommands`

Map of command names to command definitions:

```cue
#ForestCommands: {
    [string]: #ForestCommand
}
```

### `#ForestCommand`

A single command definition:

```cue
#ForestCommand: {
    description: string
    input:  {...}   // Input schema (open struct)
    output: {...}   // Output schema (open struct)
}
```

| Field | Type | Description |
|-------|------|-------------|
| `description` | `string` | Human-readable description |
| `input` | struct | Input parameters schema |
| `output` | struct | Output schema |

### `#ForestHooks`

Map of hook contract names to hook definitions:

```cue
#ForestHooks: {
    [string]: #ForestHook
}
```

### `#ForestHook`

A hook contract. Open struct — components define the methods:

```cue
#ForestHook: {
    ...  // Methods defined by the component
}
```

---

## Deployment Contract

Defined in `forest.sh/contracts/deployment@v0`. Components implementing this contract can be used as deployment targets.

```cue
#DeploymentHooks: {
    prepare: {
        input: {}
        output: {
            manifests: [...string]
        }
    }
    release: {
        input: {
            release_id: string
        }
        output: {}
    }
    rollback: {
        input: {
            release_id:      string
            target_revision: string
        }
        output: {}
    }
}
```

| Hook | Description |
|------|-------------|
| `prepare` | Generate deployment manifests. Returns list of manifest file paths. |
| `release` | Apply the deployment. Receives the release ID for tracking. |
| `rollback` | Roll back to a previous version. Receives target revision. |
