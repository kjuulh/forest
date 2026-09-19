# Product direction

Forest's product is the **express path from code to a running release—on Forest
Runtime or customer-owned infrastructure**.

The private component exchange is how platform teams package the reusable
capabilities behind that path. The release control plane is how application
teams use them to ship code. Neither is a side feature:

```text
versioned components                         application repository
build + deploy + policy + tools              code + forest.cue + forest.lock
            \                                      /
             +---------- forest release ---------+
                              |
                   prepare → record → orchestrate
                              |
                 +------------+-------------+
                 |                          |
          Forest Runtime             deployment provider
        managed application          customer's Kubernetes,
             runtime                 cloud, GitOps, or service
```

## Customer and job

The initial customer is a platform team serving roughly 10–100 developers. It
already maintains internal CLIs, CI templates, scripts, build logic, and one or
more deployment systems.

Its application teams need to:

- get from a source revision to a running release without learning every
  target-specific API;
- use the same release command for a managed runtime and their own
  infrastructure;
- preserve the option to bring infrastructure and credentials under their
  control;
- inspect and lock the exact build, deployment, policy, and tooling contracts;
- see who released what, where it went, what approved it, and whether it
  converged.

Forest succeeds when a new application can adopt approved components and
complete a first observable release in minutes, while an existing application
can retain its current infrastructure behind a provider.

## Product pillars

### Component Exchange

A component is a versioned executable capability with a CUE contract,
platform-specific artifacts, commands, optional tool metadata, and deployment
hooks. Teams consume it as:

1. a typed and locked repository or CI dependency through `forest run`;
2. a verified developer tool through `forest global add`;
3. a build or deployment provider used during `forest release`.

### Release Control Plane

`forest release` prepares application artifacts, records source and actor
context, selects destinations, applies policy and approval gates, assigns work,
streams rollout state, and retains history.

The intended fast path is one command:

```bash
forest release create --environment dev
```

The project and destination configuration—not a different release command—
choose where the application runs.

## Deployment modes

### Forest Runtime

Forest operates the application runtime and gives teams the shortest path to a
running service. The current implementation coordinate is
`forage/containers@1`: it submits a container-service resource to a
Forage-managed cluster and follows the rollout.

This is a private-preview implementation, not yet a production hosting
guarantee. Production Forest Runtime requires hardened workload isolation,
tenant boundaries, image provenance, secrets, networking, observability,
capacity controls, billing meters, rollback, and an operated SLO.

### Customer infrastructure through providers

A destination can point at infrastructure the customer owns. Built-in
destination types cover Flux, Kubernetes, and Terraform. `forest/generic@1`
delegates releases to an external service implementing the versioned
`forest.provider.v1.DestinationProvider` gRPC protocol.

This preserves customer control of infrastructure and cloud spend while Forest
provides the release record, policies, pipelines, approvals, credentials
exchange, and status model. Provider execution is a first-class product path,
not an enterprise escape hatch.

## Target product boundary

The first supported product boundary must contain:

- the `forest` CLI and named server contexts;
- CUE component contracts and generated SDK bindings;
- permanently immutable component coordinates and per-platform artifacts;
- the private registry and S3-compatible artifact storage;
- exact dependency resolution and a lock file used by runtime execution;
- typed project commands and global developer tools;
- release preparation, immutable annotations, destination selection, event
  history, status, pipelines, policies, approvals, cancellation, and rollback;
- one supported Forest Runtime application profile;
- one documented provider protocol and at least one customer-infrastructure
  provider;
- authenticated, isolated runners with release-scoped credentials;
- browser/password authentication for humans and scoped workload identities
  for CI, runners, and providers;
- an operator-supported web surface for projects, destinations, components,
  and releases.

## Non-goals for the first paid product

- a public marketplace for arbitrary third-party binaries;
- a general cloud platform with managed databases and a large add-on catalogue;
- every deployment provider, cloud, or CI vendor;
- charging for deployment work performed entirely on customer-owned runners;
- running untrusted native components directly on developer machines;
- enterprise compliance claims before independent evidence exists.

Forest Runtime is intentionally an application deployment path, not a promise
to replace every cloud primitive. Provider support remains deliberately small:
one stable protocol is more valuable than many shallow integrations.

## Product principles

1. **Release is the outcome.** Component distribution matters because it makes
   the path to a running, observable application reusable and controlled.
2. **One release contract, two runtime choices.** Managed Forest Runtime and
   customer infrastructure differ in ownership and billing, not in the user's
   release workflow.
3. **Provider-backed, not provider-bound.** Deployment-specific behaviour lives
   behind a versioned provider contract. Application projects keep portable
   release intent.
4. **Immutable and inspectable.** A published coordinate must never resolve to
   different bytes. Consumers inspect contracts, checksums, provenance, and
   requested capabilities before use. Current administrative
   unpublish-and-reuse behaviour violates this target.
5. **Locked by default.** Runtime execution uses the checked-in lock and
   artifact hashes. Background work may propose updates but cannot silently
   change a repository lock.
6. **Trust is explicit.** Local native execution is trusted mode. Managed jobs
   and release work use declared capabilities and fail-closed isolation.
7. **Pay for the boundary you use.** Forest Runtime consumption is billable
   because Forest operates compute. Customer-owned execution is not metered as
   Forest compute.
8. **No security theatre.** Checksums, signing, provenance, scanning,
   authorization, and sandboxing solve different risks.

## Validation milestones

### External design-partner preview

- one neutral application completes component adoption and its first release
  from a clean checkout in under 15 minutes;
- the same application releases through Forest Runtime and one
  customer-infrastructure provider without changing the release command;
- one neutral component supplies typed build or deployment behaviour on Linux
  and macOS;
- only approved publishers and exact artifact digests can execute;
- runner identity, provider identity, artifact ownership, permanent
  coordinates, supply-chain verification, and single-tenant sandbox gates pass;
- scoped workload identities replace shared personal CI tokens;
- three design partners can use isolated deployments without
  repository-specific maintainer knowledge.

### Paid beta

- at least two independent organisations release applications weekly;
- at least one uses Forest Runtime and at least one uses a
  customer-infrastructure provider;
- every managed customer workload and release job fails closed into its
  required isolation profile;
- release rollback, provider outage, backup/restore, and negative tenant tests
  pass;
- billing separates active developers and control-plane storage from managed
  runtime consumption; customer-owned execution is not charged as compute.

### General availability

- upgrade and rollback rehearsals cover two consecutive releases;
- release and runtime SLOs are measured for at least 30 days;
- incident response, vulnerability handling, runtime operations, and support
  ownership are staffed;
- an external security assessment has no unresolved critical or high findings;
- pricing has been validated through paid design partners.

## Success measures

| Measure | Target before paid beta |
|---|---|
| Time to first release | Clean application checkout reaches a healthy preview destination in under 15 minutes |
| Runtime portability | Same release declaration succeeds on Forest Runtime and one provider-backed customer destination |
| Weekly release adoption | At least 2 independent organisations release weekly for 8 consecutive weeks |
| Publish integrity | 100% of component versions are immutable, checksummed, signed, and attributable |
| Release traceability | 100% of production releases bind source revision, actor, destination, provider/runtime, and terminal status |
| Resolution reproducibility | Clean checkout resolves the same lock and artifact hashes |
| Support load | Fewer than 1 maintainer intervention per organisation per month |
| Pricing signal | At least 2 partners accept the internal pricing hypothesis or sign a paid beta |

## Related decisions

- [Pricing strategy](pricing.md)
- [Security and sandboxing](security-and-sandboxing.md)
- [Productization readiness](readiness.md)
- [Destinations](../concepts/destinations.md)
- [Releases](../concepts/releases.md)
- [Architecture](../architecture/index.md)
