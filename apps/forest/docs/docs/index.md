# Forest

**Release applications quickly on Forest Runtime or your own infrastructure.**

Forest combines a release control plane with a private component exchange.
Platform teams publish versioned build, deployment, policy, and tooling
capabilities. Application teams lock those contracts and use one release
workflow across managed Forest Runtime and provider-backed destinations.

> **Private preview**
>
> Release orchestration, Forest Runtime, and providers exist, but they are not
> ready for mutually untrusted tenants or production hosting guarantees. Native
> components currently inherit the invoking process's host access. Use only
> trusted publishers, runners, providers, and operators, and read
> [Security and sandboxing](product/security-and-sandboxing.md).

## The Forest delivery path

```text
                  private component exchange
             build + deploy + policy + tool contracts
                                |
                   application + forest.lock
                                |
                     forest release create
                                |
              prepare → annotate → policy → pipeline
                                |
                 +--------------+--------------+
                 |                             |
          Forest Runtime              deployment provider
        managed application         customer-owned platform
```

The target supported product boundary includes:

- CUE-defined component inputs, outputs, commands, tool metadata, and deployment
  hooks;
- generated Rust and TypeScript SDK bindings;
- private, immutable publication of versioned artifacts;
- checksum verification and lock files consumed by execution;
- release preparation, source annotation, destination selection, pipelines,
  approval gates, status, rollback, and event history;
- a managed Forest Runtime application path;
- built-in and external deployment providers for customer-owned infrastructure;
- organisation-scoped discovery, policy, and audit;
- typed repository/CI commands and verified developer-tool installation.

The codebase contains these surfaces today at different maturity levels. The
[readiness plan](product/readiness.md) states which isolation, identity,
reliability, and support gates must pass before each becomes a customer
guarantee.

## Start here

1. [Install the private preview](getting-started/installation.md)
2. [Configure authentication](getting-started/authentication.md)
3. [Complete the Forest quickstart](quickstart.md)
4. [Release an application](getting-started/first-release.md)
5. [Understand destinations and providers](concepts/destinations.md)
6. [Author a component](guides/authoring-components.md)

## Productization

- [Product direction](product/index.md) defines the customer, product boundary,
  non-goals, and validation milestones.
- [Pricing](product/pricing.md) separates control-plane pricing, managed
  runtime consumption, and customer-owned execution; billing is not active.
- [Security and sandboxing](product/security-and-sandboxing.md) records the
  current trusted-code boundary and required execution profiles.
- [Readiness](product/readiness.md) gives the ordered gates for preview, paid
  beta, and general availability.

## Technical reference

- [CLI reference](reference/cli.md)
- [Configuration files](reference/configuration-files.md)
- [Architecture](architecture/index.md)
- [CI/CD integration](guides/ci-cd.md)
