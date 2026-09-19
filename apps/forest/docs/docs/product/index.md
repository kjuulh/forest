# Product direction

Forest's product wedge is a **private component exchange for platform teams**.

A team publishes a versioned executable capability once, then consumes it as:

1. a typed and locked dependency invoked by repositories and CI with `forest run`;
2. a verified developer tool installed with `forest global add`.

This is narrower than “developer platform” and more useful than a generic binary registry. The same artifact carries a CUE contract, platform builds, commands, optional tool metadata, and a versioned coordinate.

## Customer and job

The initial customer is a platform team serving roughly 10–100 developers. It already maintains internal CLIs, CI templates, scripts, or reusable build logic and has these problems:

- capabilities are copied between repositories and drift;
- local tooling and CI automation use different implementations;
- upgrades are untracked or require repository-wide campaigns;
- consumers cannot inspect a stable typed interface before execution;
- private distribution, provenance, and revocation are assembled ad hoc.

Forest succeeds when that team can publish once, adopt from a second clean repository, and keep both CI and developer workstations on intentional versions.

## Target product boundary

The first supported product boundary must contain:

- the `forest` CLI and named server contexts;
- CUE component contracts and generated SDK bindings;
- permanently immutable component coordinates and per-platform artifacts;
- the private registry and S3-compatible artifact storage;
- exact dependency resolution and a lock file used by runtime execution;
- `forest run` command dispatch;
- `forest global` tool and catalogue installation;
- browser/password authentication and personal preview tokens;
- scoped machine identities and audit events before external organisations use
  CI publication.

The repository also contains release pipelines, deployment destinations, the web application, notifications, and operational overlays. They remain preview features until each has an explicit owner, threat model, support contract, and isolation story.

## Non-goals for the first paid product

- a public marketplace for arbitrary third-party binaries;
- hosted general-purpose compute priced by execution minute;
- a Heroku replacement or managed database product;
- every deployment provider and CI vendor;
- running untrusted native components directly on developer machines;
- enterprise compliance claims before independent evidence exists.

Keeping these out prevents Forest from competing simultaneously with package registries, CI engines, PaaS products, and cloud control planes.

## Product principles

1. **One artifact, two surfaces.** Typed automation and developer tools remain facets of the same component rather than separate packaging systems.
2. **Immutable and inspectable.** A published coordinate must never resolve to
   different bytes; consumers can inspect its contract, platforms, checksums,
   provenance, and requested capabilities before installing it. Current
   administrative unpublish-and-reuse behaviour violates this target and is a
   launch blocker.
3. **Locked by default.** Projects and workstations resolve explicit versions,
   and runtime execution uses the checked-in lock and artifact hashes.
   Background updates may propose or fetch, but must not silently change a
   repository lock.
4. **Trust is explicit.** Native execution is labelled trusted mode. Sandboxed execution has declared capabilities and deny-by-default boundaries.
5. **Customer compute stays customer compute.** Forest charges for the managed exchange and control plane, not for executions on customer-owned runners.
6. **Private before public.** Organisation-scoped exchange and policy precede any public ecosystem.
7. **No security theatre.** A checksum proves identity, not safety. Signing, provenance, scanning, sandboxing, and authorization solve different risks and are documented separately.

## Validation milestones

### External design-partner preview

- one neutral example component publishes Linux and macOS artifacts;
- a second clean checkout completes publish → add/lock → typed run → global
  install in under 15 minutes;
- only approved first-party publishers and exact artifact digests can execute;
- runner identity, artifact ownership, permanent coordinates, supply-chain
  verification, and single-tenant sandbox gates pass;
- scoped machine identities replace shared personal CI tokens;
- three design partners can use isolated deployments without
  repository-specific knowledge.

### Paid multi-organisation beta

- at least two independent organisations use both component surfaces weekly;
- every customer-controlled managed job fails closed into a disposable hardened
  sandbox; no in-process fallback exists;
- audit export, backup/restore, and negative tenant tests pass;
- billing can meter active developers, storage, and egress without metering
  customer-run execution.

### General availability

- upgrade and rollback rehearsals cover two consecutive releases;
- published SLOs are measured for at least 30 days;
- incident response, vulnerability handling, and support ownership are staffed;
- an external security assessment has no unresolved critical or high findings;
- pricing has been validated through paid design partners rather than page conversion alone.

## Success measures

| Measure | Target before paid beta |
|---|---|
| Time to first value | New team publishes and consumes from a second repository in under 15 minutes |
| Dual-surface adoption | At least 2 of 3 design partners use both `forest run` and `forest global` |
| Weekly retained organisations | At least 2 independent organisations for 8 consecutive weeks |
| Publish integrity | 100% of versions immutable, checksummed, signed, and attributable |
| Resolution reproducibility | Clean checkout resolves the same lock and artifact hashes |
| Support load | Fewer than 1 maintainer intervention per organisation per month |
| Pricing signal | At least 2 partners accept the internal pricing hypothesis or sign a paid beta |

## Related decisions

- [Pricing strategy](pricing.md)
- [Security and sandboxing](security-and-sandboxing.md)
- [Productization readiness](readiness.md)
- [Architecture](../architecture/index.md)
