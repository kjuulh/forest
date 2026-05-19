# 015: Trigger immediate Flux reconciliation after release

## Problem

After `forest release create` pushes manifests to the flux git repo, flux polls on a 5-minute interval. During development, this means every config change requires either:

1. Waiting up to 5 minutes, or
2. Manually running `flux reconcile source git flux-system && flux reconcile kustomization <name>`

This breaks the development flow and makes iterating slow.

## Current state

The flux destination handler already supports a `reconcile_url` metadata field and has code to trigger reconciliation via a Flux Receiver webhook. But:

1. The Receiver isn't set up by default
2. The `reconcile_url` isn't populated in most setups
3. There's no guidance on how to configure it

## Desired behavior

After a successful release, if the destination has webhook integration configured, immediately trigger reconciliation so manifests are applied within seconds.

If not configured, print a helpful message:

```
Release completed. Flux will reconcile within 5m.
Tip: configure 'reconcile_url' in the destination metadata for immediate reconciliation.
```

## Implementation plan

### Phase 1: Auto-generate Flux Receiver

When the flux destination is first created (or on first release), automatically create a Flux Receiver CR in the target cluster:

```yaml
apiVersion: notification.toolkit.fluxcd.io/v1
kind: Receiver
metadata:
  name: forest-release
  namespace: flux-system
spec:
  type: generic
  secretRef:
    name: forest-receiver-token
  resources:
    - kind: GitRepository
      name: flux-system
```

The receiver URL and token are then stored in the destination metadata for subsequent releases.

### Phase 2: Trigger on release

The `FluxV1Handler::run()` already calls `trigger_reconciliation()` when `reconcile_url` is set. Ensure this is wired up end-to-end.

### Phase 3: CLI feedback

After the webhook is triggered, wait a few seconds and report whether the source was fetched:

```
Release completed. Triggered Flux reconciliation.
Source fetched: main@sha1:abc123
```

## Files to change

- `crates/forest-runner/src/destinations/fluxv1.rs` — ensure receiver setup and trigger work
- `crates/forest-server/src/destinations/fluxv1.rs` — auto-create Receiver CR on first release
- Destination metadata schema — document `reconcile_url` and `webhook_secret`
