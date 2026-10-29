# Components

Components are the building blocks of Forest. They are self-contained, versioned plugins that provide commands, deployment hooks, and configuration schemas.

## What Is a Component?

A component is:

- A **binary** (Rust, Go, or Docker) that implements the Forest SDK protocol
- A **CUE spec** (`forest.component.cue`) that defines its configuration schema, commands, and hooks
- **Templates** (optional) for generating deployment manifests

Components are published to the Forest registry and consumed by projects as dependencies.

## Component Structure

```
my-component/
  forest.cue              # Component metadata (name, version, upload config)
  forest.component.cue    # Spec schema, commands, and hooks
  cue.mod/
    module.cue            # CUE module definition
  crates/my-component/    # Implementation (Rust example)
    Cargo.toml
    src/
      main.rs             # Entry point
      forestgen.rs        # Generated SDK code
  templates/              # Deployment templates (optional)
    deployment/
      forest/kubernetes@1/
        deployment.yaml
        service.yaml
```

## Spec Schema

The spec defines what consuming projects must provide:

```cue
#Spec: sdk.#ForestSpec & {
    name:      string & =~"^[a-z][a-z0-9-]*$"
    namespace: string | *"default"
    image:     string
    replicas:  int & >=1 & <=100 | *1
    ports: [...#Port]
    health_checks: #HealthChecks
    env_vars: [...#EnvVar]
}
```

CUE's type system enforces constraints — ranges, patterns, defaults, and optional fields — at validation time.

## Commands

Commands are named operations the component exposes:

```cue
#Commands: sdk.#ForestCommands & {
    prepare: {
        description: "Generate Kubernetes manifests"
        input: {}
        output: {
            manifests: [...string]
        }
    }
    status: {
        description: "Check deployment status"
        input: {}
        output: {
            ready:   int
            desired: int
            healthy: bool
        }
    }
    validate: {
        description: "Validate spec and manifests"
        input: {}
        output: {
            valid:  bool
            errors: [...string]
        }
    }
}
```

Users invoke commands via `forest run`:

```bash
forest run prepare
forest run status
forest run my-component:validate  # Fully qualified
```

## Hooks

Hooks are lifecycle callbacks that Forest invokes automatically during operations like deployments:

```cue
#Hooks: sdk.#ForestHooks & {
    "forest/deployment": sdk.#ForestHook & {
        prepare: {
            description: "Generate manifests for deployment"
            input: {}
            output: { manifests: [...string] }
        }
        release: {
            description: "Apply manifests to target"
            input: { release_id: string }
            output: {}
        }
        rollback: {
            description: "Roll back to previous revision"
            input: {
                release_id:      string
                target_revision: string | *""
            }
        }
    }
}
```

Forest recognises these hook contracts:

| Contract | Purpose |
|----------|---------|
| `forest/deployment` | Deploy, rollback, and prepare manifests |
| `forest/observability` | Configure monitoring and logging |
| `forest/security` | Image scanning and network policies |

## SDK Protocol

Components implement the `ComponentService` trait:

```rust
pub trait ComponentService<S>: Send + Sync {
    fn call(
        &self,
        method: &str,
        spec: &S,
        input: serde_json::Value,
        context: &CallContext,
    ) -> impl Future<Output = Result<serde_json::Value, Error>>;

    fn methods(&self) -> Vec<MethodDescriptor>;
    fn template_config(&self) -> TemplateConfig;
}
```

The `CallContext` provides runtime information:

- `project`, `organisation`, `environment`
- `release_id`, `work_dir`
- `dry_run` flag

## Templates

Components can include file templates that get rendered during `forest release prepare`. Templates live in `templates/deployment/{destination_type}/`:

```
templates/
  deployment/
    forest/kubernetes@1/
      deployment.yaml
      service.yaml
      ingress.yaml
```

Forest renders these with the project's spec values and copies them to the working directory.

## Sharing

A component is private to its organisation until an admin of that organisation makes it public:

```bash
forest components visibility <org>/<name> public
```

Anyone signed in can then depend on it and download it, and anyone, even without signing in, can see it (readme, versions, manifest) on the public registry pages. It is read-only to them: publishing still needs membership, and changing visibility needs a human admin of the organisation (app tokens are refused). `private` reverses it.

An admin can make a whole organisation public instead, for one whose every component is meant to be shared:

```bash
forest organisation visibility <org> public
```

That opens all its components, current and future, to reads, and nothing else about the organisation: projects, releases, destinations and members stay members-only.

Both settings are also in Forage, under the organisation's **Settings → Visibility** and each project's **Settings → Visibility**. A component is public when its own setting or its organisation's is, so a component set to private is still public while its organisation is.

Visibility governs the registry API. The CUE module registry that `cue` imports component packages from serves their CUE files without authentication, so never put secrets in a component's CUE.

## Lifecycle

1. **Author** — Write the component spec and implementation
2. **Build** — `forest run build` compiles for the configured platforms
3. **Publish** — `forest publish` uploads binary + spec to the registry
4. **Consume** — Projects add the component with `forest add org/name`

See the [Authoring Components](../guides/authoring-components.md) guide for a full walkthrough.
