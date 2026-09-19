# Forest

**A private component exchange for platform teams.**

Publish a versioned executable capability once, then use it in two places:

- as a typed, locked dependency in a repository or CI workflow;
- as a verified, lazily installed developer tool.

Forest combines a CUE-defined interface, native component binaries, a private
registry, project dependency locking, and release orchestration. The same
component can expose typed automation through `forest run` and a tool facet
through `forest global add`.

> [!WARNING]
> Forest is a private development preview, not a security boundary. Component
> binaries and installed tools execute with the invoking user's privileges;
> global-tool warming can execute a binary and persist its output into future
> shells. Runner enrolment and artifact lifecycle authorization also have known
> multi-tenant blockers. Use only trusted publishers, runners, operators, and
> non-sensitive fixtures on a trusted private network. See
> [Security and sandboxing](apps/forest/docs/docs/product/security-and-sandboxing.md).

## Why Forest

Platform capabilities usually fragment into CI snippets, shell scripts,
container images, internal CLIs, and documentation that drift independently.
Forest gives them one versioned distribution and execution contract:

1. an author defines typed inputs, outputs, and commands in CUE;
2. CI builds platform-specific binaries and publishes versioned artifacts;
3. a project resolves and locks the component;
4. developers and CI invoke the same typed command;
5. tool-shaped components can also install a checksum-verified shim on `PATH`.

The first product boundary is deliberately narrower than the full repository:
the **Component Exchange** comprises the CLI, component protocol and SDK,
registry, artifact storage, resolver, lock file, and global-tool experience.
Hosted deployment orchestration, destinations, and the web application remain
preview capabilities until their operational and security contracts are ready.

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

## Current capabilities

| Surface | Current capability |
|---|---|
| Component contracts | CUE schemas with generated Rust and TypeScript bindings |
| Registry | Private component metadata, manifests, files, and per-platform binaries |
| Project consumption | Exact registry versions, `forest.lock`, local-path development, typed command dispatch |
| Developer tools | Per-tool installs, organisation catalogue subscriptions, lazy verified downloads, shell integration |
| Authentication | Browser device login, password login, contexts, personal access tokens |
| Releases | Annotation, destinations, pipelines, approval gates, rollback, event history |
| Web application | Organisation, project, component, release, and account management |

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

Pricing is not active and future paid prices have not been set. The public
[pricing strategy](apps/forest/docs/docs/product/pricing.md) defines the
candidate active-developer meter and validation gates. Concrete figures remain
an internal hypothesis in [`design/PRICING.md`](design/PRICING.md) until paid
design partners and observed unit economics validate them.

## Licence

No public licence has been selected. Until a licence file is added, this
repository is private and all rights are reserved. Do not redistribute it or
represent it as open source.
