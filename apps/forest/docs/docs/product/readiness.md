# Productization readiness

Forest has substantial working software. Productization is the work required to make its boundary, safety, operation, price, and support promises true for customers other than its original authors.

This plan is ordered by risk. Features do not compensate for an unsafe repository, ambiguous trust model, or unowned service.

## Stage 0: safe private baseline

Exit criteria before inviting external design partners:

| Workstream | Required outcome |
|---|---|
| Repository | Historical credentials inventoried and rotated; current repository remains private; clean export procedure rehearsed |
| Legal | Licence selected; copyright and third-party notices reviewed |
| Product boundary | CLI, SDK, component exchange, release control plane, Forest Runtime adapter, and provider protocol build without private deployment overlays |
| Documentation | One neutral quickstart completes component lock → prepare → release to both a managed runtime destination and a provider-backed customer destination |
| Authentication | Human access works for trusted internal development; personal-token limitations are explicit; external CI remains disabled |
| Artifact integrity | Coordinate-reuse and end-to-end checksum gaps are documented as blockers |
| Execution | Native execution, descriptor probes, warming, shell capture, and sourcing are labelled trusted-only |
| Operations | Named owner, logs, basic metrics, backup, restore instructions, and rollback procedure exist |
| Repository health | Pinned toolchain builds; SQLx offline metadata is current; deterministic tests do not depend on order or shared state |

## Stage 1: design-partner preview

Goal: validate that Forest makes releasing applications materially faster while
preserving a real choice between managed and customer-owned infrastructure.

Required:

- three independent organisations each receive an isolated single-tenant Forest
  deployment; any managed runtime is tenant-isolated and no shared
  multi-tenant control plane is used;
- one neutral application releases through Forest Runtime and one external
  provider with the same project declaration and release command;
- Forest Runtime has an explicit workload contract for image identity,
  resources, environment, secrets, networking, health, rollout, logs, and
  rollback;
- the provider protocol is versioned; provider endpoints are operator-allowed,
  mutually authenticated, and receive audience-bound, release-scoped
  credentials;
- runner enrolment, capabilities, streams, assignments, and release tokens use
  scoped workload identity and reject replay or revocation;
- every artifact lifecycle operation derives ownership server-side and passes
  negative cross-tenant tests;
- component coordinates permanently reject conflicting bytes and verification
  covers upload, storage read, download, extraction, and cache use;
- signatures, provenance, SBOMs, scanning/quarantine, revocation, and trusted
  builder policy ship;
- managed component and release jobs use the sandboxed single-tenant profile and
  fail closed with no in-process server fallback;
- Forest Runtime application workloads run in isolated namespaces/workloads
  with enforced resource, network, secret, and tenant boundaries;
- CI uses scoped expiring machine identities, not shared personal tokens;
- global-tool probes, warming, capture, and sourcing require approval for the
  exact publisher and digest;
- invitation, deletion/export, audit, quota, runtime-limit, and abuse workflows
  exist;
- protocol, manifest, lock, release, provider, upgrade, and compatibility
  policies are documented;
- support intake and weekly review of activation/release blockers are staffed;
- billing is inactive; preview limits and lack of hosting/control-plane SLA are
  shown in the product.

Exit when at least two partners release applications weekly for eight weeks,
one partner uses Forest Runtime, one uses a customer-infrastructure provider,
and each can onboard a second repository without maintainer intervention.

## Stage 2: paid beta

Required before accepting payment:

| Area | Required capability |
|---|---|
| Managed job sandboxing | Mandatory disposable gVisor/microVM-equivalent sandbox per customer-controlled component or release job; attested scheduling fails closed |
| Forest Runtime | Workload isolation, image admission, secrets, network policy, health, autoscaling limits, logs, rollback, region/capacity policy, and measured runtime SLO |
| Providers | Versioned protocol, mutual workload identity, endpoint policy, release-token audience binding, idempotency, retries, cancellation, status reconciliation, and revocation |
| Supply chain | Stage 1 controls are monitored, revocation reaches locked consumers and active releases, and signing/scanning outages fail closed |
| Identity | Machine/runner/provider-token inventory, rotation and revocation; SSO/SCIM only if sold; no personal-token sharing |
| Tenancy | Negative suite across APIs, runtime workloads, providers, runner streams, release credentials, object/cache keys, logs, and background workers |
| Reliability | Control-plane and runtime SLO instrumentation, alerts, provider/runtime outage rehearsal, migrations, backup/restore proof, capacity limits, and status communication |
| Billing | Active-developer, artifact, and Forest Runtime meters; customer-owned execution exclusion; invoice preview; entitlements; billing-provider webhook idempotency |
| Support | Published response targets, named control-plane/runtime on-call owner, incident severity model, and rollback authority |
| Security | Independent assessment with no unresolved critical/high findings and tracked remediation for lower findings |

Use the [pricing strategy](pricing.md) to validate the meter before making a
paid design-partner offer. Do not publish price points or promise Business or
Enterprise capabilities until their evidence and readiness gates pass.

## Stage 3: general availability

Required:

- 30 days of measured SLO performance at or above the published target;
- two consecutive upgrade and rollback rehearsals from supported versions;
- documented API, protocol, manifest, and lock compatibility windows;
- customer-visible status, incident history, data export, and deletion workflows;
- disaster recovery exercise meeting stated RPO/RTO;
- dependency and base-image patch SLO compliance;
- support rotation staffed beyond a single maintainer;
- signed terms, privacy policy, data-processing terms, subprocessors, and regional data decisions;
- pricing and gross margin validated with paying customers.

## Engineering workstreams

### Product boundary and extraction

- move generic component-exchange, release-control-plane, runtime-adapter, and
  provider-protocol code into an allow-listed tree;
- remove hard-coded Understory and Rawpotion hosts, catalogues, workflows, and
  account data;
- separate private deployment overlays from reusable runtime/provider contracts;
- define ownership for CLI, registry, release orchestration, Forest Runtime,
  provider protocol, runner, web application, and billing state;
- create a fresh repository rather than making the historical repository
  public.

### Sandboxing and runner protocol

- add signed capability declarations to component manifests;
- enforce capabilities in the runner rather than trusting component code;
- isolate each invocation and ensure cleanup after timeout, crash, and cancellation;
- make secret access explicit, short-lived, auditable, and command-scoped;
- add hostile-component fixtures for filesystem, network, fork bomb, memory, disk, output, and timeout escape attempts;
- retain a clearly labelled trusted-native profile for local tools requiring host integration.

See [Security and sandboxing](security-and-sandboxing.md).

### Release orchestration, runtime, and providers

- define one portable release declaration shared by Forest Runtime and provider
  destinations;
- make release preparation bind source, lock, artifacts, actor, configuration,
  provider/runtime version, and destination set;
- make every release step idempotent and reconcile requested state with
  independently observed runtime/provider state;
- specify cancellation, timeout, retry, approval, rollback, and partial-failure
  semantics;
- productionize the Forest Runtime workload contract, isolation, secrets,
  networking, logs, health, scaling, capacity, and regional operations;
- version the external provider protocol and ship a conformance suite;
- authenticate providers with scoped workload identity and audience-bound
  release tokens; fail closed on endpoint or identity mismatch;
- keep provider-backed customer infrastructure a first-class path in
  quickstarts, support, telemetry, and pricing.

### Security cleanup

- rotate historical credentials and introduce push protection;
- review every authorization endpoint and cross-tenant data path;
- replace broad service-account bypass with scoped machine identities;
- sign releases and publish provenance/SBOMs;
- harden web sessions, OAuth, CSRF, redirects, request limits, and log redaction;
- establish vulnerability intake, patch targets, and incident exercises.

### Registry and artifact lifecycle

- guarantee immutability transactionally across metadata and object storage;
- make interrupted uploads resumable or safely abortable;
- define yanking, revocation, retention, garbage collection, and legal hold;
- verify signatures and checksums before cache admission and every execution;
- make platform availability and compatibility visible before install;
- support organisation quotas without partial or corrupt publications.

### Identity and tenancy

- define roles and permissions as a versioned authorization matrix;
- add organisation-scoped machine identities with minimal scopes and expiry;
- provide token/session inventory, last-used time, rotation, and revocation;
- partition cache keys, object paths, event subjects, and background jobs by tenant;
- test enumeration resistance and resource ownership independently of URL or request labels.

### Reliability and operations

- choose supported control-plane, runtime, and provider deployment topologies
  and version skew;
- automate database migrations with backward-compatible rollout rules;
- define SLOs for authentication, registry reads, publication, artifact
  download, release scheduling/status, provider calls, and Forest Runtime;
- instrument latency, error rate, queue depth, storage, runtime capacity,
  provider health, and tenant saturation;
- rehearse backup/restore, region loss, provider outage, runtime capacity
  exhaustion, signing-key compromise, bad release, and dependency outage;
- pin production images by digest and keep rollback artifacts available.

### Distribution and compatibility

- publish signed CLI artifacts for Linux amd64/arm64 and macOS arm64/amd64;
- provide a stable authenticated installation channel and checksummed installer;
- version the component protocol, CUE schema, generated SDKs, manifest, and lock file;
- document deprecation periods and supply migration tooling;
- add clean-machine compatibility tests across supported CLI/server version pairs.

### Pricing and billing

- validate willingness to pay before implementing a broad billing system;
- meter active developers, artifact storage/egress, and Forest Runtime
  consumption from immutable events;
- keep customer-owned provider execution and CI minutes unmetered as Forest
  compute;
- enforce entitlements server-side and make control-plane, artifact, and runtime
  usage separately exportable;
- model control-plane margin and managed-runtime margin independently against
  included allowances, support, and operations.

See [Pricing](pricing.md).

### Documentation and support

- maintain one tested quickstart that ends in a healthy release and remove
  fictional or obsolete commands;
- document Forest Runtime and customer-provider paths with the same application
  project;
- generate or verify CLI reference against `forest --help`;
- document current versus planned capabilities on every commercial page;
- provide operator installation, runtime/provider setup, upgrade, backup,
  restore, capacity, and incident runbooks;
- publish security, contribution, governance, release, provider, runtime, and
  support policies;
- make all examples provider-neutral unless demonstrating one explicitly.

## Current concrete cleanup items

This documentation pass completed:

- aligned the root, CLI workspace, MkDocs home, and web landing page around the
  combined Component Exchange and Release Control Plane;
- made Forest Runtime and customer-infrastructure providers the two first-class
  deployment modes;
- removed obsolete PaaS pricing claims and separated the control-plane
  subscription, managed-runtime consumption, and customer-owned execution;
- corrected primary examples for the removed `forest build` command, explicit
  usage blocks, exact versions, and current personal-token limitation;
- added contribution, vulnerability-reporting, pricing, sandboxing, and staged
  readiness documentation.

Stage 0 still includes:

- remove or clearly quarantine historical Understory/private CI examples from
  the generic CI/CD guide;
- make runtime project resolution consume the checked-in lock for version
  constraints, or support exact versions only;
- prohibit coordinate reuse after administrative unpublish;
- authenticate runner enrolment and assignment streams, and enforce ownership
  on every artifact lifecycle operation;
- prevent unapproved global-tool warm, descriptor probes, shell capture, and
  sourcing from executing a new digest silently;
- add missing acceptance-test SQL queries to checked-in SQLx offline metadata;
- remove shared-runtime/order sensitivity from the full server acceptance suite;
- define and test the Forest Runtime workload contract beyond the current basic
  container-service translation;
- add provider protocol conformance, authentication, idempotency, cancellation,
  reconciliation, and negative authorization tests;
- resolve inherited/generated repository-wide formatting drift.

These are product risks because they affect reproducibility, isolation,
installation, security posture, and operating cost.

## Explicitly deferred

Until the paid-beta gates pass, do not prioritize:

- public third-party marketplace discovery;
- managed databases or a broad cloud add-on catalogue;
- billing per release or for customer-owned provider execution;
- broad provider catalogue expansion before the generic protocol and one
  customer-infrastructure provider are stable;
- compliance badges without independently reviewed controls;
- a plugin API separate from the component and provider protocols.

The next implementation should reduce a named readiness risk or prove that an
application can move through the same release workflow on Forest Runtime and a
customer provider. Everything else is backlog.
