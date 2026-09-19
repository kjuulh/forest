# Forest

**Publish a platform capability once. Use it as typed automation and as a developer tool.**

Forest is a private component exchange for platform teams. Components combine a
CUE contract with versioned, platform-specific executable artifacts. A project
can lock and invoke a component through `forest run`; a developer can install
the same component's tool facet through `forest global add`.

> **Private preview**
>
> Forest is not ready for untrusted component execution. Native components
> currently inherit the invoking process's host access. Use only components
> published by organisations you trust, and read
> [Security and sandboxing](product/security-and-sandboxing.md).

## The component exchange

```text
                          versioned component coordinate
                    CUE contract + manifest + artifacts
                                      |
                 +--------------------+--------------------+
                 |                                         |
        project / CI dependency                     developer tool
       forest.add + forest.lock                 forest global add
                 |                                         |
         forest run <command>                      verified lazy shim
```

The initial supported product boundary includes:

- CUE-defined component inputs, outputs, commands, and tool metadata;
- generated Rust and TypeScript SDK bindings;
- private publication of versioned artifacts, with permanent coordinate
  immutability required before external preview;
- checksum verification and project lock files;
- organisation-scoped discovery and catalogues;
- typed command execution in repositories and CI;
- lazy, verified developer-tool installation and shell integration.

Release pipelines, deployment destinations, and the managed web application
exist in this repository but remain preview surfaces until their isolation,
tenancy, reliability, and support gates pass.

## Start here

1. [Install the private preview](getting-started/installation.md)
2. [Configure authentication](getting-started/authentication.md)
3. [Complete the Component Exchange quickstart](quickstart.md)
4. [Understand components](concepts/components.md)
5. [Author a component](guides/authoring-components.md)

## Productization

- [Product direction](product/index.md) defines the customer, product boundary,
  non-goals, and validation milestones.
- [Pricing](product/pricing.md) proposes a concrete managed-control-plane model;
  billing is not active.
- [Security and sandboxing](product/security-and-sandboxing.md) records the
  current trusted-code boundary and required execution profiles.
- [Readiness](product/readiness.md) gives the ordered gates for preview, paid
  beta, and general availability.

## Technical reference

- [CLI reference](reference/cli.md)
- [Configuration files](reference/configuration-files.md)
- [Architecture](architecture/index.md)
- [CI/CD integration](guides/ci-cd.md)
