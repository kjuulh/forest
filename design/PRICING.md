# Pricing proposal

> **Status:** commercial hypothesis for design-partner validation. Billing is not active, no plan is currently sold, and existing preview users must not be charged from this document.

Forest should charge for the managed component exchange and control plane. It should **not** charge per component execution when the component runs on customer-owned developer machines or CI runners. Per-run billing would punish adoption, require invasive telemetry, and price a cost Forest does not incur.

## Billing metric

The primary meter is one distinct Forest human user ID per organisation per UTC
calendar month that successfully performs at least one allow-listed private
exchange event:

- resolve private component metadata;
- publish a component version;
- download a private artifact;
- install or update a private global tool;
- change component policy.

Sign-in alone, failed requests, public browsing, local cache hits, machine
identities, and requests made while an account is suspended do not count.
Linked login identities deduplicate to the stable Forest user ID. This
definition and its canonical event names must be visible in-product and
reproducible from immutable audit events. An invoice that cannot be reconciled
from an organisation's export is a billing bug.

## Proposed plans

Prices are USD, excluding tax. The figures are private hypotheses to test with
paid design partners; they must not appear on the preview pricing page.

| Plan | Price | Intended customer | Included |
|---|---:|---|---|
| Preview | $0, invite-only | Design partners evaluating fit | Up to 10 active developers, 10 GB artifacts, community support, no SLA; CI uses dedicated least-privileged personal tokens until machine identities ship |
| Team | $20 per active developer/month, 5-developer minimum | Platform teams adopting the managed exchange | Unlimited private components and projects, 1 machine identity per billed developer after that capability ships, 50 GB artifacts, 100 GB monthly egress, 90-day audit retention, standard support |
| Business | $40 per active developer/month, 20-seat minimum | Multiple teams requiring governance | Team features, SSO/SCIM, policy controls, 500 GB artifacts, 1 TB monthly egress, one-year audit retention and export, priority support, 99.9% control-plane SLA after GA |
| Enterprise | From $30,000/year | Regulated, dedicated, or self-hosted environments | Contracted seats, dedicated or customer-managed deployment, private networking, regional controls, longer retention, negotiated SLA and support |

For the monthly Team hypothesis:

```text
monthly charge = $20 × max(5, active developers in that UTC month)
included machine identities = billed developer quantity
```

Do not offer an annual discount until committed quantity, proration, and true-up
rules are selected.

Planned overages after paid beta:

- artifact storage: **$0.25/GB-month** above the included amount;
- internet artifact egress: **$0.12/GB** above the included amount;
- additional machine identities: **$5/month** each on Team and Business.

No surprise overages during preview. Enforce a soft limit and contact the organisation before introducing a billable meter. Usage caps, notifications, and invoice previews must exist before overage charging.

## Why not the old pricing model

The previous page priced RAM, vCPU hours, databases, projects, and environments. Forest does not currently sell a general-purpose hosted compute or managed database service. Advertising those limits sells unimplemented capacity and confuses the component exchange with a PaaS.

The revised model follows the delivered value:

- seats price collaboration, policy, and support;
- storage and egress track actual registry cost;
- customer-owned execution remains unmetered;
- dedicated infrastructure and operational commitments are contracted separately.

## Packaging rules

1. Core versioning, checksums, lock files, and security updates are never premium gates.
2. Sandboxing is a safety baseline for managed execution, not an enterprise add-on.
3. SSO, SCIM, extended retention, private networking, and dedicated deployments are valid higher-tier capabilities because they create incremental operating cost.
4. Do not cap component count or project count. Such limits discourage the reuse Forest is meant to increase.
5. A machine identity must not consume a human seat, but unlimited machine identities would invite unbounded credential and audit cost.
6. The CLI and protocol need a licence decision independent of the hosted plan. “Source visible” is not a pricing strategy.

## Required billing implementation

Do not integrate Stripe before these domain rules exist:

- immutable usage events with organisation, actor type, meter, quantity, and timestamp;
- deterministic monthly aggregation and replay;
- active-developer deduplication and machine-identity exclusion;
- storage byte-hours and egress byte counters from the artifact path;
- plan entitlements enforced server-side, never only in the web UI;
- invoice preview and CSV/JSON usage export;
- idempotent provider webhook processing;
- grace period, delinquency, downgrade, deletion, and export policies;
- tax, currency, refund, and data-retention decisions;
- administrators can set budget alerts and hard egress caps.

Billing state belongs to the managed web/control-plane boundary. The component protocol, resolver, and local runner must not depend on Stripe or a billing vendor.

## Validation plan

Before publishing prices:

1. Interview at least five platform leads about current internal-tool distribution cost and budget ownership.
2. Ask three design partners to choose between the Team proposal, a lower seat price with usage fees, and an annual platform fee.
3. Obtain at least two paid-beta commitments at or above the Team effective price.
4. Measure storage, egress, control-plane compute, database, observability,
   backups, signing/scanning, payment fees, support time, and allocated
   operations/on-call cost for eight weeks.
5. Set a monthly Preview cost and egress ceiling before inviting partners.
6. Model per-plan contribution margin at expected use and full included-
   allowance use. Require at least 80% for Team and Business.
7. Require every dedicated Enterprise quote to clear an explicit contracted
   contribution-margin floor.
8. Revisit plan names and minimums; preserve the active-developer meter unless
   evidence shows procurement cannot accept it.

Pricing is validated by signed willingness to pay and observed cost, not by agreement that a pricing table “looks reasonable.”
