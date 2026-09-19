# Productization readiness

Forest has substantial working software. Productization is the work required to make its boundary, safety, operation, price, and support promises true for customers other than its original authors.

This plan is ordered by risk. Features do not compensate for an unsafe repository, ambiguous trust model, or unowned service.

## Stage 0: safe private baseline

Exit criteria before inviting external design partners:

| Workstream | Required outcome |
|---|---|
| Repository | Historical credentials inventoried and rotated; current repository remains private; clean export procedure rehearsed |
| Legal | Licence selected; copyright and third-party notices reviewed |
| Product boundary | CLI, SDK, registry, artifact storage, resolver, lock file, and global tools build without private deployment code |
| Documentation | One neutral quickstart completes publish → add/lock → typed run → global install from a clean machine |
| Authentication | Human access works for trusted internal development; personal-token limitations are explicit; external CI remains disabled |
| Artifact integrity | Coordinate-reuse and end-to-end checksum gaps are documented as blockers |
| Execution | Native execution, descriptor probes, warming, shell capture, and sourcing are labelled trusted-only |
| Operations | Named owner, logs, basic metrics, backup, restore instructions, and rollback procedure exist |
| Repository health | Pinned toolchain builds; SQLx offline metadata is current; deterministic tests do not depend on order or shared state |

## Stage 1: design-partner preview

Goal: validate that the Component Exchange solves a repeated problem, not merely that the software can run.

Required:

- three independent organisations each receive an isolated single-tenant
  deployment; no shared multi-tenant control plane;
- runner enrolment, capabilities, streams, assignments, and release tokens use
  scoped workload identity and reject replay or revocation;
- every artifact lifecycle operation derives ownership server-side and passes
  negative cross-tenant tests;
- component coordinates permanently reject conflicting bytes and verification
  covers upload, storage read, download, extraction, and cache use;
- signatures, provenance, SBOMs, scanning/quarantine, revocation, and trusted
  builder policy ship;
- managed execution uses the sandboxed single-tenant profile and fails closed
  with no in-process server fallback;
- CI uses scoped expiring machine identities, not shared personal tokens;
- global-tool probes, warming, capture, and sourcing require approval for the
  exact publisher and digest;
- one neutral Rust component has a typed command and tool facet on Linux and
  macOS;
- invitation, deletion/export, audit, quota, and abuse-limit workflows exist;
- protocol, manifest, lock, upgrade, and compatibility policy are documented;
- support intake and weekly review of activation blockers are staffed;
- billing is inactive; preview limits and lack of SLA are shown in the product.

Exit when at least two partners use both `forest run` and `forest global` weekly for eight weeks and can onboard a second repository without maintainer intervention.

## Stage 2: paid beta

Required before accepting payment:

| Area | Required capability |
|---|---|
| Sandboxing | Mandatory disposable gVisor/microVM-equivalent sandbox per customer-controlled invocation; attested scheduling fails closed |
| Supply chain | Stage 1 controls are monitored, revocation reaches locked consumers, and signing/scanning outages fail closed |
| Identity | Machine-token inventory, rotation and revocation; SSO/SCIM only if sold; no personal-token sharing |
| Tenancy | Negative suite across APIs, runner streams, release credentials, object keys, cache keys, logs, and background workers |
| Reliability | SLO instrumentation, alerts, migration rehearsal, backup/restore proof, capacity limits, and status communication |
| Billing | Active-developer, storage, and egress meters; invoice preview; entitlements; provider webhook idempotency |
| Support | Published response targets, named on-call owner, incident severity model, and rollback authority |
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

- move generic component-exchange code into an allow-listed tree;
- remove hard-coded Understory and Rawpotion hosts, catalogues, workflows, and account data;
- separate private deployment overlays from reusable component and registry packages;
- define ownership for CLI, registry, web application, runner, and billing state;
- create a fresh repository rather than making the historical repository public.

### Sandboxing and runner protocol

- add signed capability declarations to component manifests;
- enforce capabilities in the runner rather than trusting component code;
- isolate each invocation and ensure cleanup after timeout, crash, and cancellation;
- make secret access explicit, short-lived, auditable, and command-scoped;
- add hostile-component fixtures for filesystem, network, fork bomb, memory, disk, output, and timeout escape attempts;
- retain a clearly labelled trusted-native profile for local tools requiring host integration.

See [Security and sandboxing](security-and-sandboxing.md).

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

- choose supported deployment topology and version skew;
- automate database migrations with backward-compatible rollout rules;
- define SLOs for authentication, registry reads, publication, artifact download, and control-plane API;
- instrument latency, error rate, queue depth, storage, and tenant saturation;
- rehearse backup/restore, region loss, signing-key compromise, bad release, and dependency outage;
- pin production images by digest and keep rollback artifacts available.

### Distribution and compatibility

- publish signed CLI artifacts for Linux amd64/arm64 and macOS arm64/amd64;
- provide a stable authenticated installation channel and checksummed installer;
- version the component protocol, CUE schema, generated SDKs, manifest, and lock file;
- document deprecation periods and supply migration tooling;
- add clean-machine compatibility tests across supported CLI/server version pairs.

### Pricing and billing

- validate willingness to pay before implementing a broad billing system;
- meter active developers, storage, and egress from immutable events;
- keep customer-owned execution unmetered;
- enforce entitlements server-side and make usage exportable;
- model support and infrastructure cost against the included allowances.

See [Pricing](pricing.md).

### Documentation and support

- maintain one tested quickstart and remove fictional or obsolete commands;
- generate or verify CLI reference against `forest --help`;
- document current versus planned capabilities on every commercial page;
- provide operator installation, upgrade, backup, restore, and incident runbooks;
- publish security, contribution, governance, release, and support policies;
- make all examples provider-neutral unless explicitly labelled as private integrations.

## Current concrete cleanup items

This documentation pass completed:

- aligned the root, CLI workspace, MkDocs home, and web landing page around the
  Component Exchange;
- removed the PaaS pricing claims from the live pricing and usage pages;
- corrected the primary examples for the removed `forest build` command,
  explicit usage blocks, exact versions, and current personal-token limitation;
- added contribution, vulnerability-reporting, pricing-strategy, sandboxing,
  and staged-readiness documentation.

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
- resolve inherited/generated repository-wide formatting drift.

These are product risks because they affect reproducibility, isolation,
installation, security posture, and operating cost.

## Explicitly deferred

Until the paid-beta gates pass, do not prioritize:

- public third-party marketplace discovery;
- managed databases or general-purpose application hosting;
- usage billing for customer-run commands;
- broad provider catalogue expansion;
- compliance badges without independently reviewed controls;
- a plugin API separate from the component protocol.

The next implementation should reduce a named readiness risk or prove dual-surface adoption. Everything else is backlog.
