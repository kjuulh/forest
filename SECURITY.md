# Security policy

## Supported status

Forest is a private preview. There is no supported public release or public security-update window yet. Security fixes are applied to the active private development line; older commits and private preview builds may not receive backports.

The project must define supported versions and patch timelines before general availability.

## Reporting a vulnerability

Do not open a public issue, paste a credential into chat, or include an exploit payload in ordinary CI logs.

During the private preview, report vulnerabilities to **contact@kjuulh.io** with:

- `Forest security report` in the subject;
- affected component and version or commit;
- impact and required preconditions;
- minimal reproduction steps;
- whether the issue is known to be actively exploited;
- a safe way to contact you.

Send only the minimum evidence needed to establish the issue. Request an encrypted channel before sending credentials, customer data, or a working exploit.

Expected initial acknowledgement: three business days. This is a target, not an SLA. If no acknowledgement arrives, send one follow-up without adding sensitive material.

## Scope

Reports are especially useful for:

- cross-organisation authorization or data exposure;
- authentication, OAuth, token, or session bypass;
- artifact substitution, signature, checksum, lock-file, or cache-integrity failures;
- registry path traversal or object-key confusion;
- component or runner sandbox escape;
- unintended host filesystem, network, credential, or secret access;
- command injection through manifests, templates, component inputs, or release metadata;
- server-side request forgery, unsafe redirects, CSRF, XSS, or webhook forgery;
- denial of service that crosses documented resource limits;
- secrets in current source, artifacts, images, logs, or generated documentation.

The current native execution mode is explicitly trusted-code-only. A component doing what its trusted publisher intentionally implemented is not by itself a sandbox escape. A component bypassing an enforced sandbox or obtaining undeclared capabilities is in scope.

## Coordinated handling

The maintainer will:

1. acknowledge and establish a private channel;
2. reproduce and assign severity;
3. contain exposed credentials or affected services first;
4. develop and verify a fix without disclosing reporter details;
5. rotate compromised secrets and invalidate affected artifacts or sessions;
6. agree on disclosure timing when external users are affected;
7. publish remediation and attribution when a public release exists.

No bug bounty is currently offered. Good-faith research that avoids privacy violations, destructive actions, persistence, and service disruption will be handled as coordinated disclosure.

## Security model

See [Security and sandboxing](apps/forest/docs/docs/product/security-and-sandboxing.md) for the current trust boundary, planned execution profiles, supply-chain controls, and launch gates.
