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

## TypeScript / Deno Components

Most Forest components are implemented in TypeScript and run via Deno. This section covers the TypeScript-specific workflow.

### Scaffold

A typical TypeScript component has the following directory structure:

```
my-component/
  forest.cue              # name, organisation, dependencies
  forest.component.cue    # Spec, Commands, Hooks
  cue.mod/module.cue      # CUE module
  deno.json               # Import map (@forest/sdk)
  src/
    main.ts               # Implementation
    forestgen.ts           # Generated (do not edit)
    deps/                  # Generated dependency clients
```

The CUE files (`forest.cue`, `forest.component.cue`, `cue.mod/module.cue`) follow the same conventions as Rust components. The key difference is the runtime: instead of a compiled binary, the component runs as a Deno process.

### Generate TypeScript Code

Run the code generator targeting TypeScript:

```bash
forest generate --output ./src/ --language typescript
```

This produces:

- `src/forestgen.ts` — typed interfaces matching your `#Spec`, input/output types for each command and hook, the router dispatch logic, and handler type signatures.
- `src/deps/<org>_<name>.ts` — a typed client for each declared dependency. These clients wrap the `callComponent()` protocol so you can invoke other components with full type safety.

Re-run this command whenever you change `forest.component.cue`.

### Implement the Component

In `src/main.ts`, import the generated types and implement each handler:

```typescript
import {
  type Spec,
  type PrepareInput,
  type PrepareOutput,
  type StatusInput,
  type StatusOutput,
  runOnce,
} from "./forestgen.ts";

runOnce({
  async status(spec: Spec, _input: StatusInput): Promise<StatusOutput> {
    return {
      healthy: true,
      message: `${spec.name} is running`,
    };
  },

  async "forest/deployment/prepare"(
    spec: Spec,
    _input: PrepareInput,
  ): Promise<PrepareOutput> {
    const manifests = generateManifests(spec);
    return { manifests };
  },

  async "forest/deployment/release"(spec: Spec, input) {
    await applyDeployment(spec, input.releaseId);
    return {};
  },
});
```

The `runOnce()` function reads a single request from stdin, dispatches it to the matching handler, and writes the response to stdout. This is the standard execution model for Forest components.

### Dependencies and callComponent

Components can invoke other components through generated dependency clients. Each file in `src/deps/` exposes functions that call the target component's commands via the `callComponent()` protocol.

For example, if your component depends on `my-org/forage-s3`, the generator creates `src/deps/my_org_forage_s3.ts` with typed functions for each of that component's commands. You call them like any async function:

```typescript
import { createBucket } from "./deps/my_org_forage_s3.ts";

const result = await createBucket({ name: "my-bucket", region: "eu-west-1" });
```

For dependency resolution to work at runtime, the parent project must list every dependency — including transitive ones — in its own `forest.cue`. If component A depends on component B, and a project uses A, the project must also declare B.

### Build and Test

Build the component to generate `meta.json` (the component manifest):

```bash
forest build
```

For local testing, use a path-based dependency in your consuming project, just as with Rust components:

```bash
forest add my-org/my-component --path ../my-component
```

This lets you iterate without publishing.

## Template Authoring

### Jinja2 Templates

Components that generate deployment manifests use Jinja2 templates. Place template files in a directory structure that matches the destination type:

```
templates/deployment/forest/flux@1/
  05-crds.yaml.jinja2
  10-namespace.yaml.jinja2
  15-rbac.yaml.jinja2
  30-deployment.yaml.jinja2
```

The numeric prefixes control rendering order. Files are processed in lexicographic order, so `05-crds.yaml` is rendered before `30-deployment.yaml`.

### Available Variables

Inside templates, the following variables are available:

| Variable | Description |
|----------|-------------|
| `config.*` | The resolved spec values from the consuming project. Fields match the `#Spec` definition. |
| `env` | The environment name (e.g., `dev`, `staging`, `prod`). |

### Available Filters

Forest extends Jinja2 with these custom filters:

| Filter | Description | Example |
|--------|-------------|---------|
| `to_lower` | Lowercase | `{{ name \| to_lower }}` |
| `to_upper` | Uppercase | `{{ name \| to_upper }}` |
| `to_snake` | snake_case | `{{ name \| to_snake }}` |
| `to_camel` | camelCase | `{{ name \| to_camel }}` |
| `to_pascal` | PascalCase | `{{ name \| to_pascal }}` |
| `to_screaming_snake` | SCREAMING_SNAKE_CASE | `{{ name \| to_screaming_snake }}` |
| `to_kebab` | kebab-case | `{{ name \| to_kebab }}` |
| `as_bool` | Convert to boolean string | `{{ flag \| as_bool }}` |
| `dictsort` | Sort a dictionary by key | `{{ dict \| dictsort }}` |

### The `is configured` Test

Forest provides a custom Jinja2 test called `is configured`. Use this instead of standard truthiness checks when testing whether an optional config block is present.

The problem: in Jinja2, `{% if config.foo %}` evaluates to false for empty objects (`{}`). This is incorrect for Forest's use case — an empty block like `forage_postgresql: {}` means "use this component with defaults", not "this component is absent".

```jinja2
{# WRONG — fails for empty objects like forage_postgresql: {} #}
{% if config.forage_postgresql is defined and config.forage_postgresql %}

{# CORRECT — true for any non-undefined, non-none value including {} #}
{% if config.forage_postgresql is configured %}
```

Always prefer `is configured` for optional component blocks in your templates.

## Sealed Secrets

Forest supports sealing Kubernetes secrets using the `forest run seal` command. Sealed secrets are encrypted with a cluster-specific certificate and can be safely committed to version control.

### Seal Workflow

1. **Obtain the certificate.** The `cert` parameter is a **file path** to the PEM certificate on disk, not the certificate content itself. Download it from the cluster or use a shared location.

2. **Follow the correct order.** Create your components and templates first, then seal secrets, then release. The typical workflow is:

   ```bash
   # Seal a secret value
   forest run seal --env dev --key DATABASE_URL --value "postgres://..." --cert ./certs/sealed-secrets.pem
   ```

3. **Multi-line values.** For multi-line secret values (such as credentials files or PEM certificates), use `@-` (stdin) or `@<path>` (file) syntax:

   ```bash
   # Read from stdin
   cat creds.txt | forest run seal --env dev --key NATS_CREDS --value @- --cert cert.pem

   # Read from file
   forest run seal --env dev --key NATS_CREDS --value @/path/to/creds.txt --cert cert.pem
   ```

## Common Pitfalls

1. **snake_case vs camelCase in forestgen.ts** — The generated TypeScript code uses camelCase for field names, but the runtime JSON payloads use snake_case. If you need to access a field that does not match the generated interface (or if you encounter missing data), cast the spec and use the snake_case key directly:

   ```typescript
   const value = (spec as Record<string, unknown>)["field_name"];
   ```

   This mismatch is a known issue in the code generator.

2. **Empty `#Commands` breaks `forest generate`** — If your component defines no commands (only hooks), the code generator may fail. In this case, write `forestgen.ts` manually or define a no-op command as a workaround.

3. **`{% if config.foo %}` is false for `{}`** — As described in the Template Authoring section, always use `{% if config.foo is configured %}` to test for the presence of optional config blocks. Standard Jinja2 truthiness treats empty dicts as falsy.

4. **Projects must list transitive dependencies** — If component A calls component B via `callComponent`, every project that uses A must also declare B as a dependency. Forest does not auto-resolve transitive dependencies at the project level.

5. **`forest build` does not regenerate codegen** — The `forest build` command compiles the component and produces `meta.json`, but it does not re-run code generation. When you change `forest.component.cue`, you must run `forest generate` separately before building.

## First Deployment Workflow

When deploying a new service for the first time, follow this order:

1. **Create the deployment project.** Set up `forest.cue` with the project metadata, list all components (including transitive dependencies), and create the template directory structure.

2. **Generate and build each component.** For every component in the project:

   ```bash
   forest generate --output ./src/ --language typescript
   forest build
   ```

3. **Seal any required secrets.** Encrypt sensitive values before the first release:

   ```bash
   forest run seal --env dev --key SECRET_KEY --value "..." --cert ./certs/sealed-secrets.pem
   ```

4. **Release.** Create a release for the target environment:

   ```bash
   forest release create --env dev
   ```

   This single command runs the full prepare, annotate, and release flow.

5. **Wait for reconciliation.** If using Flux, either trigger a manual reconciliation or wait for the next poll interval. The manifests generated by the release will be picked up and applied to the cluster.
