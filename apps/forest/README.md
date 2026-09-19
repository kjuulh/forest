# Forest CLI, control plane, and component SDK

This workspace implements the executable core of
[Forest](../../README.md): a release control plane and private component
exchange for platform teams.

The primary product workflow is:

```text
publish build/deploy capabilities → lock them in an application
                                      |
                           forest release create
                                      |
                    +-----------------+------------------+
                    |                                    |
             Forest Runtime                   customer infrastructure
                                         through a deployment provider
```

The same component system also powers typed `forest run` commands and verified
`forest global` developer tools. Release orchestration is a first-class product
surface: components define reusable delivery behaviour, while destinations,
pipelines, policies, approvals, and runners move application code to a managed
or customer-owned runtime.

> [!WARNING]
> Components currently execute as native child processes with the permissions,
> filesystem access, environment, and network access of the invoking CLI or
> runner. Global-tool warming can also execute a tool to capture shell output
> that later shells source. Treat every component and shell integration as
> trusted code. Checksums detect corruption; they do not make an untrusted
> binary safe.

## Workspace map

| Area | Responsibility |
|---|---|
| `crates/forest` | User-facing CLI |
| `crates/forest-server` | Registry, authentication, organisations, releases, scheduling |
| `crates/forest-runner` | Remote component and release execution |
| `crates/forest-sdk` | Rust component protocol and authoring API |
| `crates/forest-sdk-codegen` | CUE-to-language bindings |
| `crates/forest-grpc-interface` | Protobuf and generated gRPC types |
| `components/` | First-party components and build providers |
| `examples/` | Runnable project and component examples |
| `docs/` | MkDocs source |
| `templates/` | Development Compose stack and container builds |

## Prerequisites

- Rust `1.98.1` and components declared in `mise.toml`
- CUE
- Docker with Compose for local services
- `mise` for repository tasks

```bash
mise install
cargo build --locked --workspace
```

## Install the CLI

Prebuilt releases support glibc-based Linux on x86_64 and arm64:

```bash
curl -fsSL https://src.rawpotion.io/rawpotion/forest/releases/latest/download/install.sh | bash
forest --version
```

To build from this checkout instead:

```bash
cargo install --path crates/forest --locked
forest --version
```

The hosted Forest service remains a private preview and requires an account.

## Start a local control plane

The development stack provides PostgreSQL, NATS, and MinIO:

```bash
cp .env.example .env
mise run local:up
mise run dev
```

`forest-server` applies embedded SQLx migrations at startup and listens on
`http://localhost:4040` by default. In another terminal:

```bash
forest context create local --server http://localhost:4040 --use
forest auth register
forest auth status
```

Stop the dependency stack and remove its data with:

```bash
mise run local:down
```

This setup is for development. It omits production TLS, external secret
management, backups, multi-tenant isolation, artifact scanning, and sandboxed
execution.

## Author and publish a component

```bash
forest components init policy-check \
  --organisation acme \
  --language rust
cd policy-check
forest add forest-contrib/build-rust@0.1.2
```

The scaffold does not expose a build command by itself. Add this usage block to
`forest.cue`:

```cue
"forest-contrib": "build-rust": {}
```

Then:

```bash
forest generate
forest run build
forest publish --dry-run
forest publish
```

`forest run build` dispatches the depended-on build component; the removed
`forest build` command must not be used. Build outputs are staged under
`.forest/component/output/<os>/<arch>/`. The publish dry run evaluates CUE,
checks the host artifact and descriptor, constructs the manifest, and shows the
target context. Server-side manifest rules run only during the real publish.

Consume the published component from another project:

```bash
forest add acme/policy-check@0.1.0
```

Add its usage block to that project's `forest.cue`:

```cue
"acme": "policy-check": {}
```

```bash
forest validate
forest run status
```

If the component declares a tool facet:

```bash
forest global add acme/policy-check@0.1.0
eval "$(forest shell zsh)"
policy-check --help
```

## Release an application

Application projects map environments to destinations. The same command targets
the managed Forest Runtime or customer infrastructure because the destination
owns the target-specific implementation:

```bash
forest release create --environment dev
forest release create --environment prod --pipeline
```

Current destination implementations include:

- `forage/containers@1` for the managed Forest Runtime preview;
- built-in Flux, Kubernetes, and Terraform integrations;
- `forest/generic@1` for an external service implementing
  `forest.provider.v1.DestinationProvider`.

These paths are active development-preview capabilities. They do not yet carry
production isolation, tenancy, availability, or support guarantees. See the
[product direction](docs/docs/product/index.md), [destinations](docs/docs/concepts/destinations.md),
and [readiness plan](docs/docs/product/readiness.md).

## Verification

Run workspace tests against the pinned lock file:

```bash
cargo test --locked --workspace
```

`forest-server` acceptance tests require live PostgreSQL, NATS, and MinIO
services plus these settings:

```bash
export DATABASE_URL=postgres://devuser:devpassword@localhost:5432/dev
export NATS_URL=nats://localhost:4222
export S3_ENDPOINT=http://localhost:9000
export S3_BUCKET=forest
export S3_ACCESS_KEY=forestdev
export S3_SECRET_KEY=forestdevpassword
export S3_REGION=us-east-1
```

SQLx release builds use checked-in offline query metadata. Query changes must
refresh `crates/forest-server/.sqlx` against a migrated development database:

```bash
mise run db:prepare
```

## Documentation

- [Documentation home](docs/docs/index.md)
- [Quickstart](docs/docs/quickstart.md)
- [Component model](docs/docs/concepts/components.md)
- [Authoring guide](docs/docs/guides/authoring-components.md)
- [CLI reference](docs/docs/reference/cli.md)
- [Architecture](docs/docs/architecture/index.md)
- [Product direction](docs/docs/product/index.md)
- [Pricing strategy](docs/docs/product/pricing.md)
- [Security and sandboxing](docs/docs/product/security-and-sandboxing.md)
- [Readiness gates](docs/docs/product/readiness.md)

Build the documentation site with:

```bash
cd docs
python -m pip install -r requirements.txt
mkdocs build --strict
```

## Product and security status

The repository is private and has no selected public licence. Historical Git
objects include credential-like values and private deployment information, so
this repository must not be made public in place. A public release requires a
fresh allow-listed history, rotated credentials, provenance and signing,
documented support ownership, and the sandboxing controls described in the
readiness plan.
