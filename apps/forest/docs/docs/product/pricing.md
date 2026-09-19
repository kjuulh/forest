# Pricing strategy

Forest is free during the invite-only private preview. Billing is not active, there is no SLA, and preview users will not be charged without a separate agreement and advance notice.

Future paid pricing has not been set. Concrete price points, allowances, and enterprise terms remain internal hypotheses until design-partner interviews, observed operating cost, and paid commitments validate them.

## What Forest should charge for

Forest should charge for the managed private component exchange and control plane:

- authenticated collaboration and organisation policy;
- private component metadata and artifact storage;
- artifact delivery;
- audit retention and export;
- enterprise identity, networking, deployment, and support commitments when those capabilities exist.

Forest should not charge per component execution when code runs on customer-owned workstations or CI runners. Per-run billing would discourage adoption, require invasive telemetry, and price compute Forest does not provide.

## Candidate billing metric

The leading candidate is an **active developer**, measured once per organisation per UTC calendar month by stable Forest human-user ID.

Only a closed allowlist of successful authenticated server events should count, such as resolving private component metadata, publishing a version, downloading a private artifact, installing or updating a private global tool, or changing component policy.

These do not count:

- sign-in by itself;
- failed requests;
- unauthenticated or public browsing;
- local cache hits that do not contact the service;
- machine identities;
- requests made while an account is suspended.

Linked login identities must deduplicate to one Forest user ID. Usage must come from immutable events and reconcile exactly with an organisation's export before this meter can appear on an invoice.

## Packaging principles

1. Core versioning, checksums, lock files, sandboxing for managed execution, and security updates are safety baselines, not premium gates.
2. Do not cap component or project counts; that discourages the reuse Forest is designed to increase.
3. Storage and internet artifact egress may have included allowances and transparent overages because they create direct marginal cost.
4. Machine identities must not consume human seats, but can have plan allowances once scoped identities actually ship.
5. SSO, SCIM, extended audit retention, private networking, dedicated deployment, and contracted support are legitimate higher-tier candidates only after they exist and pass readiness gates.
6. Customer-run component executions and CI minutes remain unmetered.
7. The CLI and protocol licence is a separate decision from hosted-service pricing.

## Before publishing prices

- Interview at least five platform leads about current internal-tool distribution cost and budget ownership.
- Ask three design partners to compare active-developer pricing, a flat annual platform fee, and a lower seat price with storage/egress overages.
- Obtain at least two paid-beta commitments at the proposed effective price.
- Measure registry storage, egress, database load, and support time for eight weeks.
- Model gross margin at the included allowances and worst credible usage.
- Define monthly minimums, annual discounts, proration, suspension, and overage formulas without ambiguity.
- Build invoice preview, usage export, budget alerts, and advance limit notifications before any automatic charge.

Pricing is validated by signed willingness to pay and observed cost, not by publishing a plausible-looking table.

## Preview promise

During private preview:

- current access is invite-only and free;
- billing and overage charging are inactive;
- there is no uptime or support SLA;
- current limits are communicated during onboarding;
- trusted internal development may use dedicated least-privileged personal CI
  tokens; external design-partner CI waits for scoped workload identities;
- native components are trusted code and are not sandboxed;
- external design partners are not admitted until the isolation, artifact,
  runner, and supply-chain gates pass.

See [Productization readiness](readiness.md) for the external-preview,
paid-beta, and general-availability gates.
