# Authoring Components

This guide walks through creating a Forest component from scratch — defining its spec, implementing the SDK protocol, building, and publishing.

## Scaffold

```bash
forest components init my-component \
  --organisation my-org \
  --language rust
```

This creates:

```
my-component/
  forest.cue              # Metadata: name, version, upload config
  forest.component.cue    # Spec schema, commands, hooks
  cue.mod/module.cue      # CUE module definition
  crates/my-component/
    Cargo.toml
    src/main.rs
```

## Define the Spec

Edit `forest.component.cue` to define what consuming projects must provide:

```cue
package my_component

import "forest.sh/forest/sdk@v0"

#Spec: sdk.#ForestSpec & {
    name:      string & =~"^[a-z][a-z0-9-]*$"
    image:     string
    replicas:  int & >=1 & <=100 | *1
    ports: [...#Port]
    env_vars: [...#EnvVar]
}

#Port: {
    name:     string
    port:     int & >0 & <=65535
    external: bool | *false
}

#EnvVar: {
    key:   string
    value: string
}
```

CUE's type system gives you:

- **Constraints**: `int & >=1 & <=100`
- **Defaults**: `| *1`
- **Patterns**: `=~"^[a-z][a-z0-9-]*$"`
- **Optional fields**: `autoscaling?: #Autoscaling`
- **Union types**: `"tcp" | "udp"`

## Define Commands

Commands are operations users invoke with `forest run`:

```cue
#Commands: sdk.#ForestCommands & {
    status: {
        description: "Check service health"
        input: {}
        output: {
            healthy: bool
            message: string
        }
    }
    validate: {
        description: "Validate configuration"
        input: {}
        output: {
            valid:  bool
            errors: [...string]
        }
    }
}
```

## Define Hooks

Hooks are callbacks Forest invokes during lifecycle events. The most important is the deployment contract:

```cue
#Hooks: sdk.#ForestHooks & {
    "forest/deployment": sdk.#ForestHook & {
        prepare: {
            description: "Generate deployment manifests"
            input: {}
            output: { manifests: [...string] }
        }
        release: {
            description: "Deploy to target"
            input: { release_id: string }
            output: {}
        }
        rollback: {
            description: "Rollback deployment"
            input: {
                release_id:      string
                target_revision: string | *""
            }
        }
    }
}
```

## Configure Metadata

Edit `forest.cue` for component metadata and build configuration:

```cue
package my_component

import "forest.sh/forest/sdk@v0"

component: sdk.#ForestComponent & {
    name:    "my-component"
    version: "0.1.0"

    upload: {
        type:   "rust"
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

## Generate SDK Code

Generate typed Rust code from your CUE spec:

```bash
forest generate --output ./crates/my-component/src/
```

This creates `forestgen.rs` with typed structs matching your `#Spec`, input/output types for each command, and the method routing boilerplate.

## Implement the Component

In `src/main.rs`, implement the `ComponentService` trait:

```rust
use forest_sdk::{ComponentService, CallContext, MethodDescriptor, TemplateConfig};

mod forestgen;
use forestgen::Spec;

struct MyComponent;

impl ComponentService<Spec> for MyComponent {
    async fn call(
        &self,
        method: &str,
        spec: &Spec,
        input: serde_json::Value,
        context: &CallContext,
    ) -> Result<serde_json::Value, forest_sdk::Error> {
        match method {
            "status" => {
                // Check service health
                Ok(serde_json::json!({
                    "healthy": true,
                    "message": format!("{} is running", spec.name)
                }))
            }
            "forest/deployment/prepare" => {
                // Generate deployment manifests
                let manifests = generate_manifests(spec, context)?;
                Ok(serde_json::json!({ "manifests": manifests }))
            }
            "forest/deployment/release" => {
                // Apply deployment
                apply_deployment(spec, context).await?;
                Ok(serde_json::json!({}))
            }
            _ => Err(forest_sdk::Error::MethodNotFound(method.to_string())),
        }
    }

    fn methods(&self) -> Vec<MethodDescriptor> {
        vec![
            MethodDescriptor::command("status"),
            MethodDescriptor::hook("forest/deployment", "prepare"),
            MethodDescriptor::hook("forest/deployment", "release"),
            MethodDescriptor::hook("forest/deployment", "rollback"),
        ]
    }

    fn template_config(&self) -> TemplateConfig {
        TemplateConfig::default()
    }
}
```

## Add Templates (Optional)

If your component generates manifests from templates, create them in `templates/deployment/{destination_type}/`:

```
templates/
  deployment/
    forest/kubernetes@1/
      deployment.yaml
      service.yaml
```

Templates are rendered with the project's spec values during `forest release prepare`.

## Build

```bash
forest build
```

This compiles for all configured architectures and stores the binaries in the content-addressable cache at `~/.cache/forest/components/bin/`.

## Test Locally

Use a path dependency in a consuming project to test without publishing:

```bash
# In the consuming project
forest add my-org/my-component --path ../my-component
```

Then run commands and releases against the local binary.

## Publish

```bash
forest publish
```

This uploads:

1. The compiled binary (per architecture)
2. CUE spec files (`forest.cue`, `forest.component.cue`)
3. A component manifest with protocol version and capabilities

The component is now available in the registry for other projects to consume.

## Versioning

Follow [semver](https://semver.org/) for component versions:

- **Patch** (0.1.1): Bug fixes, no spec changes
- **Minor** (0.2.0): New optional fields, new commands
- **Major** (1.0.0): Breaking spec changes

Consumers use version specs (e.g., `"0.1"`) that auto-resolve to the latest matching version.
