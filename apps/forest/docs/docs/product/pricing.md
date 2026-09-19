# Pricing strategy

Forest is free during the invite-only private preview. Billing is not active,
there is no SLA, and preview users will not be charged without a separate
agreement and advance notice.

Future paid pricing has not been set. Concrete prices, runtime rates,
allowances, and enterprise terms remain internal hypotheses until
design-partner interviews, measured operating cost, and paid commitments
validate them.

## What customers buy

Forest has three commercial boundaries:

1. **Release control plane and component exchange.** Versioned capabilities,
   private artifacts, projects, policies, pipelines, approvals, audit, and
   support.
2. **Forest Runtime.** Managed application compute, networking, logs, and
   operational responsibility when a release runs on Forest infrastructure.
3. **Customer-infrastructure providers.** Forest orchestrates the release, but
   the customer owns the runtime, cloud bill, and provider service.

The pricing model must keep those boundaries visible. A team choosing its own
infrastructure must not pay Forest as though Forest supplied the compute. A team
choosing Forest Runtime must pay for the real resources and operations Forest
provides.

## Candidate billing metrics

### Control plane

The leading subscription metric is an **active developer**, measured once per
organisation per UTC calendar month by stable Forest human-user ID.

Only a closed allowlist of successful authenticated events should count, such
as resolving private component metadata, publishing a version, downloading a
private artifact, creating or approving a release, inspecting private release
state, or changing component/release policy.

These do not count:

- sign-in by itself;
- failed requests;
- unauthenticated or public browsing;
- local cache hits that do not contact the service;
- machine, runner, and provider identities;
- requests made while an account is suspended.

Linked login identities must deduplicate to one Forest user ID. Usage must come
from immutable events and reconcile exactly with the organisation's export
before it can appear on an invoice.

### Forest Runtime

Managed runtime consumption needs a separate, transparent meter tied to costs
Forest actually incurs. Candidate dimensions are:

- allocated vCPU-seconds and GiB-seconds;
- persistent storage GB-months;
- internet egress;
- optional build or release-job compute when Forest operates that compute.

Do not charge per deployment, release, request, or log line. Those meters punish
healthy delivery behaviour and are hard to predict. Include a useful runtime
allowance or credit in paid plans, expose current consumption and projected
cost, and require budget alerts plus configurable hard limits before overages.

Exact runtime rates must follow workload benchmarks and infrastructure cost
modelling. Reintroducing arbitrary RAM/vCPU numbers before the runtime is
operationally measured would repeat the old pricing mistake.

### Customer-owned execution

Provider calls and release jobs executed entirely on customer-owned
infrastructure remain unmetered as Forest compute. The organisation still pays
for the release control plane, private artifacts, policy, audit, and support.
Dedicated provider connectivity or a customer-managed Forest deployment can be
contracted separately when it creates material operating cost.

## Packaging principles

1. Core versioning, checksums, lock files, release history, sandboxing for
   managed execution, and security updates are safety baselines—not premium
   gates.
2. Do not cap component, project, environment, destination, or release counts.
   Those limits discourage the delivery workflow Forest is designed to
   standardize.
3. Registry storage, artifact egress, and Forest Runtime consumption may have
   included allowances and transparent overages because they create direct
   marginal cost.
4. Customer-owned execution and CI minutes remain unmetered.
5. Machine, runner, and provider identities do not consume human seats, but may
   have plan allowances after scoped identities ship.
6. SSO, SCIM, extended audit retention, private networking, dedicated control
   planes, and contracted support are legitimate higher-tier candidates only
   after they exist and pass readiness gates.
7. Choosing customer infrastructure is a core product mode, not an enterprise
   surcharge or downgrade.
8. The CLI and protocol licence is a separate decision from hosted-service
   pricing.

## Before publishing prices

- Interview at least five platform leads about delivery-platform budget
  ownership and the value of replacing per-target release scripts.
- Ask three design partners to compare active-developer pricing, a flat annual
  platform fee, and a lower seat price with explicit runtime consumption.
- Obtain at least two paid-beta commitments at the proposed effective price.
- Operate representative services on Forest Runtime for eight weeks and measure
  compute, networking, storage, logs, control plane, support, and on-call cost.
- Measure registry storage, artifact egress, database load, provider traffic,
  and support time separately from runtime cost.
- Define included runtime credit, storage/egress allowances, monthly minimums,
  annual discounts, proration, suspension, and overage formulas.
- Build invoice preview, usage export, projected runtime cost, budget alerts,
  and advance limit notifications before automatic charging.

Pricing is validated by signed willingness to pay and observed cost, not by
publishing a plausible-looking table.

## Preview promise

During private preview:

- current access is invite-only and free;
- control-plane and Forest Runtime billing are inactive;
- there is no uptime, hosting, or support SLA;
- current control-plane and runtime limits are communicated during onboarding;
- trusted internal development may use dedicated least-privileged personal CI
  tokens; external design-partner CI waits for scoped workload identities;
- native components, release providers, and shell integrations are trusted
  code;
- Forest Runtime and external provider access remain operator-approved;
- external design partners are not admitted until the isolation, artifact,
  runner, provider, and supply-chain gates pass.

See [Productization readiness](readiness.md) for the external-preview,
paid-beta, and general-availability gates.
