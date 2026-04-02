# 012: Document and improve the first-deployment workflow

## Problem

The first deployment of a new service has a chicken-and-egg problem:

1. The deployment template includes `envFrom: secretRef: {name}-secrets`
2. The pod can't start until the SealedSecret is unsealed into a real Secret
3. You must `forest run seal` before the first `forest release create`
4. But you need the deployment project set up before `forest run seal` works

This workflow isn't documented. Developers discover it when the pod enters `CreateContainerConfigError`.

## Desired behavior

### Option A: `forest init` command for new projects

Add a command that bootstraps the minimal sealed secret structure:

```bash
cd deployment
forest init
# Creates secrets/dev.sealed-secret.yaml with empty encryptedData
# Prints: "Sealed secret structure created. Run 'forest run seal' to add secrets."
```

### Option B: Make the secrets secretRef optional on first deploy

In the service component's deployment template, add `optional: true` to the secretRef:

```yaml
envFrom:
  - secretRef:
      name: {{ config.name }}-secrets
      optional: true
```

This lets the pod start without the secret. Once the secret is created (by sealing + releasing), it's picked up on the next pod restart.

**Trade-off:** This masks configuration errors — a missing secret won't prevent startup.

### Option C: Document the workflow clearly

Add a "First Deployment" section to the component/project docs:

```
## First Deployment

1. Set up the deployment project:
   forest init (or create forest.cue manually)

2. Seal your secrets:
   forest run seal --env dev --key MY_SECRET --value "..." --cert /path/to/cert.pem

3. Create the release:
   forest release create --env dev
```

## Recommendation

Do both Option A (init command) and Option C (documentation). Option B should be a per-project choice, not a default.

## Files to change

- `crates/forest/src/cli/init.rs` (new or extend existing) — sealed secret scaffolding
- Documentation
