# Destinations and deployment providers

A destination is the configured place where an application release goes. It
binds an environment to a versioned destination type and its credentials.

Examples:

- a Forest Runtime namespace and region;
- a customer's Kubernetes cluster;
- a Flux GitOps repository;
- a Terraform workspace;
- an external deployment-provider service.

Application teams use the same `forest release` workflow for every destination.
The destination type owns target-specific planning, apply, and status behaviour;
rollback is available only where the provider/runtime implements it.

## Two deployment modes

### Forest Runtime

`forage/containers@1` is the current managed-runtime implementation. It sends a
container-service resource to a configured Forage cluster and follows the
rollout until success or failure.

This coordinate exposes implementation history; the product name is **Forest
Runtime**. It is a development preview. It currently creates a basic container
service from release and destination metadata and does not yet carry the
isolation, networking, secrets, observability, scaling, durability, or SLO
guarantees required for production hosting.

### Customer infrastructure

Customer-owned infrastructure stays behind a destination provider. Forest
retains the release record, pipeline, policy, approval, scheduling, and status
model while the provider performs the target-specific work.

Current destination types include:

| Type | Execution target |
|---|---|
| `forest/flux@1` | Customer GitOps repository and Flux-managed cluster |
| `forest/kubernetes@1` | Customer Kubernetes cluster |
| `forest/terraform@1` | Customer Terraform/OpenTofu workspace |
| `forest/generic@1` | External service implementing the Forest provider protocol |
| `forage/containers@1` | Managed Forest Runtime preview |

The list reports code that exists, not a production support matrix. Operators
must explicitly approve and configure the types they expose.

## External provider protocol

`forest/generic@1` connects to a service implementing
`forest.provider.v1.DestinationProvider`. The provider owns its metadata schema
and deployment implementation, so adding a target does not require compiling it
into Forest.

The current call direction is control plane → provider. The provider must be
reachable from `forest-server`. `FOREST_GENERIC_PROVIDER_ALLOWED_HOSTS` is
deny-by-default and must allow the endpoint before Forest will send it a
release-scoped token. A provider that can only dial outward requires the
authenticated runner model rather than the current generic-provider path.

Example:

```bash
forest destination create \
  --organisation acme \
  --name prod-ecs \
  --environment prod \
  --type forest/generic@1 \
  --metadata provider_url=https://forest-ecs-provider.internal:4060 \
  --metadata provider_token="$PROVIDER_TOKEN"
```

For generic providers, destination metadata owns settings such as accounts,
clusters, and credentials. Project configuration adds only keys the destination
does not already define; it cannot replace destination values or send
`provider_url`/`provider_token` as provider metadata. Keep credentials in
destination-sensitive metadata, never in project artifacts.

The provider receives the annotated artifact slug, source commit/reference,
release intent ID and artifact ID alongside the execution ID. PLAN and DEPLOY
can therefore correlate the same immutable artifact across executions.

Fields marked `sensitive` by `Describe` are withheld from destination listing
even if the creator did not name them in `sensitive_keys`; provider failure
withholds all metadata rather than exposing a credential. Only an
organisation admin or infrastructure service account may reveal a withheld
credential or approve/reject a plan. Ordinary members and org-scoped app
tokens cannot perform those operator actions.

Provider authentication, mTLS, identity, release-token audience binding,
revocation, callback authorization, and cross-tenant tests remain launch gates.
Do not expose an arbitrary provider URL allowlist to untrusted organisation
members.

## Creating a Forest Runtime destination

An operator with a configured runtime endpoint can create:

```bash
forest destination create \
  --organisation acme \
  --name forest-dev \
  --environment dev \
  --type forage/containers@1 \
  --metadata forage_url=https://runtime.internal.example:4050 \
  --metadata namespace=acme \
  --metadata region=eu-west-1
```

This is operator setup, not a claim that the example host exists or that the
runtime is ready for production traffic.

## Creating a customer-infrastructure destination

For a built-in type:

```bash
forest destination create \
  --organisation acme \
  --name k8s-prod-eu \
  --environment prod \
  --type forest/kubernetes@1
```

Use `forest destination types` to inspect the configured server's actual types
and required metadata:

```bash
forest destination types
```

Secrets belong in destination-sensitive metadata or an external secret
exchange. Never commit cloud credentials to `forest.cue`.

## Mapping destinations in a project

## Terraform state

A `forest/terraform@1` destination keeps state in forest-server, behind an
HTTP backend on `FOREST_TERRAFORM_V1_HOST` (internal-only in production). Every
request to it must carry a credential signed for that one state; runs get one
automatically.

| Server variable | |
|---|---|
| `FOREST_TERRAFORM_STATE_SIGNING_KEY` | Base64, at least 32 bytes. Signs state credentials. Its own secret, so rotating `ACCESS_TOKEN_SECRET_KEY` does not revoke state credentials and rotating this does not sign anybody out. **Optional:** unset, forest derives the key from `ACCESS_TOKEN_SECRET_KEY` and logs a warning at startup. Rotating it revokes every outstanding state credential, including a running apply's, so rotate when nothing is applying. |

Generate one with `openssl rand -base64 32`.

## Mapping in Configuration

`forest.cue` declares which destinations an environment targets:

```cue
env: {
    dev: {
        destinations: [
            {destination: "forest-dev", type: "forage/containers@1"},
        ]
    }
    prod: {
        destinations: [
            {destination: "prod-ecs", type: "forest/generic@1"},
        ]
    }
}
```

Destination names can use the selector forms supported by release and trigger
configuration. The destination represents a place—account, region, cluster, or
runtime namespace—while project configuration supplies application-specific
values such as service or image identity.

## Releasing

Target all destinations in an environment:

```bash
forest release create --environment dev
```

Target an explicitly named destination:

```bash
forest release release --destination k8s-prod-eu
```

Route the environment through its configured pipeline:

```bash
forest release create --environment prod --pipeline
```

## Destination state

View the latest known release state:

```bash
forest project releases \
  --organisation acme \
  --project my-service
```

Forest records what was requested and the provider/runtime's reported outcome.
Production readiness also requires independent health signals and reconciliation
so a successful API call cannot be mistaken for a healthy rollout.

## CLI commands

```bash
forest destination create --organisation acme --name k8s-dev --environment dev --type forest/kubernetes@1
forest destination update --organisation acme --name k8s-dev
forest destination delete --organisation acme --name k8s-dev
forest destination list --organisation acme
forest destination types
```

## Drift (`forest/terraform@1`)

Terraform state records what forest last applied. It doesn't tell you whether someone has changed a resource in a console since. To check:

```bash
forest release drift --environment dev              # every destination this project deployed to
forest release drift -e prod -d prod/eu-west-1/core # one destination
```

For each destination the server takes the artifact of that destination's last successful deploy and runs `terraform plan -refresh-only -detailed-exitcode -lock=false` against it. It changes nothing: no state write, no lock, so it can't block or be blocked by a release.

The exit status is `0` when in sync, `2` when something drifted (the refresh-only plan is printed), and `1` when the check couldn't run. So a scheduled CI job can alert on `2` without parsing anything.

The check runs synchronously in the request. Scheduling it and showing the result in forage are follow-ups.

