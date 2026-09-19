# Security and sandboxing

## Current trust model

Forest components are native executables. Today `forest run`, release runners,
publish-time descriptor probes, global-tool shims, and shell-integration capture
can execute component-controlled code with the host process's privileges.
In-process destination handlers can also perform privileged target-specific
operations from `forest-server`. Forest Runtime and external-provider adapters
cross into systems that receive application configuration, artifacts, and
release-scoped credentials.

Therefore:

- a SHA-256 match proves that the downloaded bytes match the registry record;
- it does **not** prove that those bytes are safe;
- CUE validation constrains configuration, not process behaviour;
- organisation membership is the intended authorization boundary, but known
  runner and artifact-service gaps mean the current server is not safe for
  mutually untrusted tenants;
- only trusted first-party components, runners, and operators may use the
  private preview.

Any page or command that implies arbitrary third-party components are safe is incorrect until the controls below ship.

Two current control-plane blockers are especially important:

- `RunnerService` bypasses the normal authentication layer. A caller can
  self-assert runner identity and capabilities before receiving scheduled
  release data. Runner registration and every stream/message need strong
  workload identity, authorization, replay protection, and revocation.
- Artifact publication binds an actor at begin-upload, while later upload,
  commit, abort, and retrieval operations rely on supplied identifiers without
  consistently reauthorizing the caller against the original organisation and
  owner. Every lifecycle operation must enforce the same ownership boundary.

Until these are fixed and adversarially tested, deploy the control plane only
on a trusted private network. “Private registry” describes visibility and
intent, not a completed hostile-tenant security guarantee.

## Assets and adversaries

Protect:

- registry credentials, refresh tokens, machine tokens, and signing keys;
- source trees, developer home directories, SSH agents, cloud credentials, and CI secrets;
- component artifacts, manifests, lock files, and provenance;
- organisation membership, private metadata, release history, destination
  configuration, provider credentials, and audit records;
- Forest Runtime workloads, namespaces, secrets, images, networks, logs, and
  control APIs;
- runner/provider hosts, control-plane services, PostgreSQL, NATS, and object
  storage;
- availability of component resolution, publication, and release workflows.

Design for malicious publishers, compromised publisher accounts, dependency substitution, tampered object storage, a hostile component process, cross-tenant API access, stolen CI credentials, vulnerable dependencies, and operator mistakes. A public arbitrary-code marketplace adds substantially more abuse and moderation work and is not an initial goal.

## Release execution boundaries

The two deployment modes have different ownership but share one control-plane
threat model.

### Forest Runtime

Forest operates the application workload and therefore owns workload isolation,
image admission, secret delivery, network policy, resource enforcement,
tenant-safe logs, health, rollout, cleanup, regional capacity, and incident
response. A sandbox for short-lived component jobs is not sufficient proof for
a long-running application runtime.

The current `forage/containers@1` adapter creates a basic container-service
resource and follows its reported rollout. Treat it as a trusted private
integration until the runtime independently enforces the controls above and
reconciles reported state with observed workload health.

### Customer-infrastructure providers

Forest may call a built-in destination handler or dial an external
`forest.provider.v1.DestinationProvider`. In the current generic path,
`FOREST_GENERIC_PROVIDER_ALLOWED_HOSTS` fails closed when unset and restricts
which endpoint Forest may dial. That is an important SSRF and token-disclosure
control, but an allowlist alone does not authenticate the provider.

Before external use:

- providers use scoped workload identity and mutually authenticated transport;
- release tokens are single-purpose, audience-bound, short-lived, revocable,
  and unusable against another destination or organisation;
- provider endpoints are operator-controlled, not arbitrary tenant URLs;
- provider configuration and responses have size, schema, log, and timeout
  bounds;
- prepare/plan/apply/status/cancel operations are idempotent and replay-safe;
- credentials for customer infrastructure stay with the provider or isolated
  runner where possible rather than transiting the general control plane;
- completion is reconciled against independently observed target state;
- provider compromise and outage have isolation, revocation, rollback, and
  incident runbooks.

Customer ownership of the cloud account does not make the provider trusted by
default. The provider receives a privileged release request and may be able to
change production infrastructure.

## Execution profiles

Forest needs explicit execution profiles rather than one ambiguous “secure” mode.

### Trusted native

Current behaviour. Components and tools run directly on the workstation or
runner. Forest can also execute tool binaries during publish descriptor probes
and during `forest global warm` to capture shell snippets. Generated shell
startup code may launch a detached quiet warm, and later shells source the
captured output. This can execute and persist publisher-controlled shell code
before the user intentionally invokes the tool.

- native mode is fastest and most compatible, but is trusted-code-only;
- every executable facet and every new artifact digest needs a recorded trust
  decision covering normal run, descriptor probe, warm/update, shell capture,
  and later sourcing;
- non-interactive paths must fail closed without prior approval;
- approval must show publisher, exact version and digest, signature status,
  requested effects, and shell-integration permission;
- the current preview must use an enforced first-party publisher/digest
  allow-list and disable unapproved capture or sourcing;
- native mode must never be described as sandboxed.

### Sandboxed single-tenant runner

Required for any externally operated CI or release execution. This profile is
acceptable only when each deployment serves one mutually trusting tenant.

Baseline Linux isolation:

- a fresh rootless OCI container or equivalent per invocation;
- non-root UID/GID, user namespace, `no_new_privileges`, all capabilities dropped;
- read-only root filesystem and an empty, size-limited temporary filesystem;
- only declared workspace paths mounted; source read-only unless write access is required;
- no Docker/containerd socket, host PID namespace, host network, SSH agent, or ambient cloud metadata access;
- seccomp and AppArmor/SELinux policy where available;
- CPU, memory, process, file-size, disk, and wall-clock limits;
- egress denied by default and enforced outside the guest; after DNS
  resolution, deny metadata, link-local, private, and control-plane
  destinations unless explicitly approved;
- secrets are short-lived, audience-scoped, explicitly approved, destroyed at
  exit, and redacted from logs as defence in depth—redaction cannot prevent a
  malicious component from transforming and exfiltrating a granted secret;
- output size and log-rate limits;
- the worker is destroyed after every invocation, including apparent success;
- scheduling fails closed when an attested sandbox is unavailable; customer
  work must never fall back to in-process execution inside `forest-server`.

A rootless container is meaningful defence in depth, not a complete
hostile-code boundary. It must not mix mutually untrusted organisations on a
shared kernel.

### Hardened managed

Required before a paid multi-organisation service accepts customer-controlled
jobs:

- Firecracker, Cloud Hypervisor, gVisor, or an equivalently reviewed isolation
  boundary;
- one tenant and one invocation per disposable sandbox;
- measured base images, no shared writable filesystem, and short-lived
  credentials;
- network policy enforced outside the guest;
- attestation checked before assignment, with no native/in-process fallback;
- host kernel patch SLO and escape-response runbook;
- admission blocked unless artifact signature, provenance, and scan policy pass.

Forest should not launch a public execution marketplace merely because basic
containers exist.

### WASI capability runtime

WASI can provide a narrower portable runtime for components that do not require arbitrary native behaviour. It is a useful future profile, not a universal replacement: compilers, Docker builds, platform CLIs, and interactive developer tools often need capabilities outside WASI.

## Capability declaration

Every executable facet should declare capabilities before installation or execution. A future schema needs, at minimum:

```cue
execution: {
    profile: "sandboxed"
    filesystem: {
        workspace: "read"
        output:    "write"
    }
    network: {
        dns: true
        egress: ["registry.example.com:443"]
    }
    secrets: ["registry-publish-token"]
    limits: {
        timeout: "10m"
        memory:  "1GiB"
        cpu:     2
    }
}
```

This is a design sketch, not a currently supported manifest. The server must validate declared capabilities; the runner must enforce them; policy must be able to reduce but never silently expand them. Undeclared access fails closed.

## Artifact supply chain

Before external preview, publication and installation need:

1. immutable `(organisation, component, version)` coordinates;
2. SHA-256 verification on upload, storage read, download, and cache use;
3. publisher identity and source revision recorded in the manifest;
4. Sigstore/cosign-compatible signatures and keyless or organisation-key verification policy;
5. SLSA-style provenance tying source, builder, inputs, and outputs together;
6. SPDX or CycloneDX SBOM per platform artifact;
7. malware and dependency scanning with quarantine before availability;
8. explicit yanking/revocation that preserves the audit record and warns locked consumers;
9. retention and deletion rules for blobs no longer referenced by reachable versions;
10. trusted builder policy for organisation-wide catalogue publication.

Do not conflate scanning with sandboxing. A clean scan can miss malicious behaviour; a sandbox limits damage when prevention fails.

## Security cleanup before any public repository

The current repository history contains credential-like values and private deployment details. Required actions:

- inventory, revoke, and rotate every historical credential with an accountable owner;
- archive the current repository privately and publish from a fresh allow-listed root;
- select a licence and confirm rights for every retained dependency and contribution;
- remove private hosts, account identifiers, deployment state, fixtures, and operational overlays from the export;
- enable pre-commit and server-side secret scanning plus push protection;
- scan the fresh Git object database, release artifacts, container layers, and generated documentation independently;
- prohibit committed `.env` files while retaining a non-secret `.env.example`;
- move production secrets to a managed secret store with rotation and audit trails;
- document vulnerability reporting and incident ownership.

History rewriting alone is insufficient: leaked credentials remain compromised and must be rotated.

## Control-plane hardening

Before any external preview:

- replace shared personal CI tokens with scoped, expiring machine identities;
- remove or tightly scope cross-organisation service-account bypasses;
- authenticate runners with scoped workload identity; authorize registration,
  capability updates, scheduling streams, release-token delivery, and result
  submission; reject replayed or revoked runner sessions;
- bind every artifact upload, chunk, commit, abort, metadata read, and download
  to the original organisation and authorized actor rather than trusting opaque
  IDs as bearer credentials;
- authorize every remaining gRPC and HTTP operation from the stored resource
  owner, not caller-supplied organisation labels;
- add negative cross-tenant tests for runner streams, release credentials,
  reads, writes, event streams, artifact lifecycle operations, object keys, and
  timing-sensitive enumeration;
- hash stored bearer credentials, rotate refresh tokens, revoke token families on reuse, and expose session/device management;
- require TLS for every production hop; authenticate internal NATS, PostgreSQL, and object-store connections;
- apply CSRF protection, secure cookies, strict redirect allow-lists, CSP, and output sanitization to the web surface;
- bound request, manifest, artifact, log, and decompression sizes;
- redact authorization headers, tokens, secrets, signed URLs, and component inputs from logs and traces;
- back up and restore PostgreSQL and object metadata together; rehearse recovery;
- publish dependency, base-image, and host-patching SLOs.

## Security acceptance gates

No external preview until:

- historical credentials are rotated and a clean repository scan passes;
- runner enrolment uses workload identity, server-issued runner IDs and
  authorized capabilities; assignment streams and release tokens are
  identity/audience-bound, replay-resistant, and revocable;
- every artifact lifecycle operation derives ownership server-side and enforces
  actor and organisation authorization;
- an authorization matrix has no unscoped endpoints and automated negative
  cross-tenant coverage passes;
- coordinates reject conflicting bytes permanently; manifest and artifact
  digests are bound and verified during upload, storage read, download,
  extraction, and cache use;
- signatures, publisher/source attribution, provenance, SBOMs,
  scanning/quarantine, revocation propagation, retention rules, and trusted
  builder policy ship;
- every native execution path—including probes, warm/update, shell capture and
  sourcing—requires an existing trust decision for the exact publisher and
  digest;
- externally operated component and release jobs use the sandboxed
  single-tenant profile and fail closed rather than running in the control
  plane;
- Forest Runtime enforces workload, tenant, secret, network, resource, image,
  log, and cleanup boundaries independently of the release adapter;
- external providers use operator-controlled endpoints, scoped workload
  identity, mutually authenticated transport, audience-bound release tokens,
  idempotent operations, and independently reconciled status;
- security contact and incident owner are named.

No paid multi-organisation beta until:

- all customer-controlled component and release jobs use the hardened managed
  profile with one disposable sandbox per invocation;
- Forest Runtime has passed workload-isolation, tenant-boundary, network,
  secret, image-admission, capacity, and incident-response exercises;
- provider conformance, compromise, outage, replay, cancellation, and negative
  cross-tenant suites pass;
- scoped machine, runner, and provider identities plus audit export ship;
- backup/restore and credential-compromise exercises pass;
- an independent reviewer has no unresolved critical or high findings.

No public untrusted execution until the hardened managed profile has survived
an isolation assessment and an escape-response exercise.
