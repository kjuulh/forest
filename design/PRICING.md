# Pricing proposal

> **Status:** commercial hypothesis for design-partner validation. Billing is not active, no plan is currently sold, and existing preview users must not be charged from this document.

Forest should charge separately for (a) the release control plane and component
exchange and (b) application compute Forest actually operates. It should
**not** charge per component execution, deployment, or CI minute when the work
runs on customer-owned infrastructure. The same release command must not hide
which party supplies the compute.

## Billing metric

The primary meter is one distinct Forest human user ID per organisation per UTC
calendar month that successfully performs at least one allow-listed private
exchange or release event:

- resolve private component metadata;
- publish or download a component version;
- install or update a private global tool;
- create, approve, inspect, or cancel a release;
- change component, destination, pipeline, or release policy.

Sign-in alone, failed requests, public browsing, local cache hits, machine
identities, and requests made while an account is suspended do not count.
Linked login identities deduplicate to the stable Forest user ID. This
definition and its canonical event names must be visible in-product and
reproducible from immutable audit events. An invoice that cannot be reconciled
from an organisation's export is a billing bug.

## Proposed plans

Prices are USD, excluding tax. The figures are private hypotheses to test with
paid design partners; they must not appear on the preview pricing page.

| Plan | Price | Intended customer | Included control-plane capability |
|---|---:|---|---|
| Preview | $0, invite-only | Design partners evaluating fit | Up to 10 active developers, 10 GB artifacts, operator-approved runtime/provider access within an agreed cap, community support, no SLA |
| Team | $20 per active developer/month, 5-developer minimum | Platform teams standardizing releases | Unlimited components, projects, destinations, and releases; 1 machine identity per billed developer after that capability ships; 50 GB artifacts; 100 GB artifact egress; 90-day audit retention; standard support |
| Business | $40 per active developer/month, 20-seat minimum | Multiple teams requiring governance | Team features, SSO/SCIM, policy controls, 500 GB artifacts, 1 TB artifact egress, one-year audit retention/export, priority support, 99.9% control-plane SLA after GA |
| Enterprise | From $30,000/year | Regulated, dedicated, or self-hosted environments | Contracted seats, dedicated or customer-managed control plane, private provider connectivity, regional controls, longer retention, negotiated SLA and support |

Forest Runtime is an optional consumption surface on every paid hosted plan, not
a separate edition. Include a useful monthly runtime credit only after
benchmarks establish its cost. Beyond the credit, bill transparent allocated
vCPU-seconds, GiB-seconds, persistent GB-months, and internet egress. The exact
rates remain unset until representative workloads run for eight weeks.

For the monthly Team hypothesis:

```text
monthly charge = $20 × max(5, active developers in that UTC month)
included machine identities = billed developer quantity
```

Do not offer an annual discount until committed quantity, proration, and true-up
rules are selected.

Planned overages after paid beta:

- artifact storage: **$0.25/GB-month** above the included amount;
- artifact internet egress: **$0.12/GB** above the included amount;
- Forest Runtime compute, storage, and egress: **rate to be determined from
  measured cost**, shown separately from artifact delivery;
- additional machine identities: **$5/month** each on Team and Business.

No surprise overages during preview. Enforce a hard agreed limit and contact
the organisation before introducing a billable meter. Usage caps,
notifications, projected runtime cost, and invoice previews must exist before
overage charging.

## Why not the old pricing model

The previous page priced arbitrary RAM, vCPU hours, databases, projects, and
environments before the managed runtime had an operated cost model. Some of
those dimensions are legitimate once Forest supplies application compute, but
inventing allowances first would still sell unmeasured capacity.

The revised model follows the delivered value and ownership boundary:

- active developers price the release control plane, collaboration, policy,
  audit, and support;
- artifact storage and egress track exchange cost;
- Forest Runtime usage tracks managed application infrastructure;
- customer-owned provider execution remains unmetered as Forest compute;
- dedicated infrastructure and operational commitments are contracted
  separately.

## Packaging rules

1. Core versioning, checksums, lock files, release history, and security updates
   are never premium gates.
2. Sandboxing and tenant isolation are safety baselines for Forest Runtime and
   managed release jobs, not enterprise add-ons.
3. SSO, SCIM, extended retention, private networking, and dedicated control
   planes are valid higher-tier capabilities because they create incremental
   operating cost.
4. Do not cap component, project, environment, destination, or release counts.
5. Customer-owned provider execution is a core mode, not an enterprise
   surcharge.
6. A machine, runner, or provider identity does not consume a human seat, but
   unlimited identities would create credential and audit cost.
7. The CLI and protocol need a licence decision independent of hosted pricing.

## Required billing implementation

Do not integrate Stripe before these domain rules exist:

- immutable usage events with organisation, actor type, meter, quantity, and timestamp;
- deterministic monthly aggregation and replay;
- active-developer deduplication and non-human-identity exclusion;
- artifact storage byte-hours and egress byte counters;
- Forest Runtime vCPU-seconds, GiB-seconds, persistent storage, and egress
  recorded by workload, organisation, region, and pricing version;
- an explicit zero-Forest-compute meter for customer-owned provider execution;
- plan entitlements enforced server-side, never only in the web UI;
- invoice preview and CSV/JSON usage export separating control plane, artifacts,
  and runtime;
- idempotent billing-provider webhook processing;
- budget alerts, hard runtime/egress caps, grace period, delinquency, downgrade,
  deletion, export, tax, currency, refund, and data-retention rules.

Billing state belongs to the managed web/control-plane boundary. The component protocol, resolver, and local runner must not depend on Stripe or a billing vendor.

## Validation plan

Before publishing prices:

1. Interview at least five platform leads about delivery-platform budget
   ownership and the value of one release path across managed and customer
   infrastructure.
2. Ask three design partners to choose between the Team proposal, a lower seat
   price with explicit runtime usage, and an annual platform fee.
3. Obtain at least two paid-beta commitments at or above the Team effective
   price.
4. Measure artifact delivery and control-plane cost separately from Forest
   Runtime compute, networking, storage, logs, support, and on-call cost for
   eight weeks.
5. Benchmark representative idle, bursty, and sustained services in each
   supported runtime region.
6. Set monthly Preview control-plane and runtime ceilings before inviting
   partners.
7. Model contribution margin for the seat subscription, artifact allowances,
   runtime credit, and consumption rates independently. Require at least 80%
   for Team/Business control-plane revenue; choose an explicit lower but
   sustainable target for managed runtime.
8. Require every dedicated Enterprise quote to clear an explicit contracted
   contribution-margin floor.
9. Revisit plan names and minimums; preserve the active-developer meter unless
   evidence shows procurement cannot accept it.

Pricing is validated by signed willingness to pay and observed cost, not by agreement that a pricing table “looks reasonable.”
