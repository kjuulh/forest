# Forest

**The express path from code to a running release—on Forest Runtime or your
own infrastructure.**

Forest combines release orchestration with a private component exchange.
Platform teams publish versioned build, deployment, policy, and developer-tool
capabilities once. Application teams consume those contracts and release code
through one workflow:

- run the application on the managed Forest Runtime;
- deploy it to customer-owned infrastructure through a Forest deployment
  provider;
- reuse the same typed capabilities in repositories, CI, and developer
  workstations.

CUE-defined interfaces, native component binaries, a private registry, project
locking, release history, pipelines, approvals, runners, and destinations form
one delivery contract. Components expose typed automation through `forest run`,
developer tools through `forest global add`, and deployment hooks used by
`forest release`.

> [!WARNING]
> Forest is a private development preview, not a security boundary. Component
> binaries and installed tools execute with the invoking user's privileges;
> global-tool warming can execute a binary and persist its output into future
> shells. Runner enrolment, provider identity, artifact lifecycle
> authorization, managed workload isolation, and tenant boundaries have known
> productization blockers. Use only trusted publishers, runners, providers,
> operators, and non-sensitive fixtures on a trusted private network. See
> [Security and sandboxing](apps/forest/docs/docs/product/security-and-sandboxing.md).

## Why Forest

The difficult part of shipping an application is rarely one deploy command. A
team must standardize builds, configuration, credentials, environments,
approvals, rollout status, and the target-specific API—then keep local tooling
and CI aligned.

Forest turns those concerns into versioned contracts:

1. a platform author publishes typed build, deployment, policy, or tool
   components;
2. an application locks the exact capabilities it uses;
3. `forest release prepare` renders the release artifact from application code
   and configuration;
4. Forest records the source revision and release intent;
5. a pipeline sends the release to either Forest Runtime or a deployment
   provider for customer-owned infrastructure;
6. Forest streams status and retains the release history.

The product has two inseparable pillars:

- **Component Exchange:** distribute the reusable capabilities that define how
  software is built, checked, and deployed.
- **Release Control Plane:** provide one fast, observable release path across
  managed Forest Runtime and customer-owned destinations.

The managed runtime is not a promise of a broad Heroku-style platform.
Customer-owned destinations are not second-class. Both implement the same
release contract, so an application can change where it runs without replacing
its release workflow.

## Core workflow

```bash
# Select a server and authenticate. The api. host lets Forest derive the web URL.
forest context create team --server https://api.forest.example.com --use
forest auth login --web

# Discover and add an exact component version to the current project.
forest components list --org acme
forest add acme/policy-check@1.4.0
```

Declare the dependency's usage block in `forest.cue` so its commands enter the
project command graph:

```cue
"acme": "policy-check": {}
```

```bash
forest validate
forest run status

# Install a tool facet from the same private catalogue.
forest global add acme/devctl@2.1.0
eval "$(forest shell zsh)"       # use `forest shell bash` or fish as needed
devctl --version                 # first run fetches and verifies the binary
```

The generated Rust component scaffold needs an explicit build provider:

```bash
forest components init policy-check --organisation acme --language rust
cd policy-check
forest add forest-contrib/build-rust@0.1.2
```

Add the build provider's usage block to `forest.cue`:

```cue
"forest-contrib": "build-rust": {}
```

Then generate, build, inspect, and publish:

```bash
forest generate
forest run build
forest publish --dry-run
forest publish
```

`forest publish --dry-run` evaluates CUE, checks the staged host binary and
descriptor, constructs the manifest, and previews the target without contacting
the registry. Server-side manifest rules run during the real publish. A live
version cannot be overwritten, but an administrator can currently unpublish and
reuse the same coordinate; permanent coordinate immutability remains a
productization gate.

## Release an application

A project maps environments to destinations in `forest.cue`. The destination
selects the execution path:

- `forage/containers@1` is the current implementation coordinate for the
  managed Forest Runtime preview;
- built-in Flux, Kubernetes, and Terraform destination types deploy to
  customer-owned infrastructure;
- `forest/generic@1` delegates a release to an external service implementing
  the versioned `forest.provider.v1.DestinationProvider` protocol.

Once an operator has configured the environments and destinations:

```bash
# Prepare, annotate, schedule, and follow the release.
forest release create --environment dev

# Route a production release through its configured pipeline and approval gates.
forest release create --environment prod --pipeline
```

The command stays the same whether the destination is Forest Runtime or a
provider. Only the destination configuration and credentials change. These
deployment paths are present in the repository but remain private-preview
capabilities until the runner, isolation, tenancy, and operational gates in the
readiness plan pass.

## Current capabilities

| Surface | Current capability |
|---|---|
| Component contracts | CUE schemas with generated Rust and TypeScript bindings |
| Registry | Private component metadata, manifests, files, and per-platform binaries |
| Project consumption | Exact registry versions, `forest.lock`, local-path development, typed command dispatch |
| Developer tools | Per-tool installs, organisation catalogue subscriptions, lazy verified downloads, shell integration |
| Release control plane | Prepare, annotate, schedule, observe, approve, reject, and record releases |
| Deployment providers | Built-in Flux, Kubernetes, Terraform, generic external-provider protocol, and destination-specific configuration |
| Forest Runtime | `forage/containers@1` translates a release into a managed container-service rollout; production isolation and operating guarantees remain gates |
| Authentication | Browser device login, password login, contexts, personal access tokens |
| Web application | Organisation, project, component, destination, pipeline, and release management |

The repository contains more than the initial supported product boundary.
Availability in source does not imply a stability, isolation, or support
guarantee. The [productization readiness plan](apps/forest/docs/docs/product/readiness.md)
records the gates for making those guarantees.

## Install the private preview

Prerequisites:

- Rust `1.98.1` as pinned by `apps/forest/mise.toml`;
- [CUE](https://cuelang.org/) for component and project evaluation;
- Git;
- access to the private Gitea repository and a Forest server.

Build the CLI from the repository:

```bash
git clone git@git.kjuulh.io:kjuulh/forest.git
cd forest
cargo install --path apps/forest/crates/forest --locked
forest --version
```

For repository development, install the pinned tools with
[mise](https://mise.jdx.dev/):

```bash
cd apps/forest
mise install
cargo build --locked --workspace
```

There is no supported anonymous binary distribution yet. Do not copy the old
Rawpotion or Understory installation snippets: they target historical release
channels and credentials.

## Local services

The server requires PostgreSQL, NATS, and S3-compatible object storage. The
development Compose file starts PostgreSQL, NATS, and MinIO:

```bash
cd apps/forest
cp .env.example .env
mise run local:up
mise run dev
```

`forest-server` applies its embedded database migrations at startup. Configure
a local CLI context separately:

```bash
forest context create local --server http://localhost:4040 --use
forest auth register
```

Local infrastructure is for development only. It does not provide production
TLS, backups, tenant isolation, artifact scanning, or sandboxed execution.

## Repository layout

| Path | Purpose |
|---|---|
| [`apps/forest/`](apps/forest/) | CLI, server, runner, SDKs, component implementations, and technical documentation |
| [`apps/forage/`](apps/forage/) | Managed web application; the crate and directory retain the historical Forage name |
| [`apps/forest-ci/`](apps/forest-ci/) | CI adapter for invoking Forest releases |
| [`deployment/`](deployment/) | Private integration and deployment overlay |
| [`.woodpecker/`](.woodpecker/) | Private Gitea/Woodpecker build and release pipelines |

## Documentation

- [Quickstart](apps/forest/docs/docs/quickstart.md)
- [Component concepts](apps/forest/docs/docs/concepts/components.md)
- [Authoring components](apps/forest/docs/docs/guides/authoring-components.md)
- [CLI reference](apps/forest/docs/docs/reference/cli.md)
- [Architecture](apps/forest/docs/docs/architecture/index.md)
- [Product direction](apps/forest/docs/docs/product/index.md)
- [Pricing strategy](apps/forest/docs/docs/product/pricing.md)
- [Security and sandboxing](apps/forest/docs/docs/product/security-and-sandboxing.md)
- [Productization readiness](apps/forest/docs/docs/product/readiness.md)
- [Security policy](SECURITY.md)
- [Contributing](CONTRIBUTING.md)

The rendered documentation site is built from `apps/forest/docs`.

## Product status

Forest is being separated from historical Understory and Rawpotion deployment
concerns. The current private repository preserves that operational history for
reconciliation, so it must not be made public in place. Publication requires a
fresh allow-listed repository, credential rotation, a chosen licence, and the
security gates in the readiness plan.

Pricing is not active and future paid prices and runtime rates have not been
set. The public
[pricing strategy](apps/forest/docs/docs/product/pricing.md) separates the
active-developer control-plane subscription, managed Forest Runtime
consumption, and unmetered customer-owned execution. Concrete figures remain an
internal hypothesis in [`design/PRICING.md`](design/PRICING.md) until paid
design partners and observed unit economics validate them.

## Licence

No public licence has been selected. Until a licence file is added, this
repository is private and all rights are reserved. Do not redistribute it or
represent it as open source.
