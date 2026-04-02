# 001: Fix Flux Kustomization `wait: true` deadlock on unhealthy deployments

## Problem

When a deployment is unhealthy (crash-looping, missing secret, bad config), the Flux Kustomization with `wait: true` blocks reconciliation indefinitely. This prevents subsequent releases from being applied — the fix that would make the deployment healthy can never land.

This creates an unrecoverable deadlock that requires manual intervention (`kubectl patch` or `flux suspend/resume`) to break.

## Current behavior

`crates/forest-runner/src/destinations/fluxv1.rs` generates:

```yaml
spec:
  wait: true
  timeout: 3m
```

When the deployment doesn't become healthy within 3m, the kustomization enters `HealthCheckFailed` state and retries, but the health check keeps failing, blocking new manifest applications.

## Temporary fix applied

Changed to `wait: false, force: true` which removes health checking entirely. This is too aggressive — we lose all release health feedback.

## Desired behavior

Releases should always apply new manifests, even when the current deployment is unhealthy. But we should still report health status back to the user.

## Implementation plan

### Option A: Two-phase reconciliation (recommended)

1. Use `wait: false` so manifests always apply immediately
2. After the flux destination commits and pushes, start a background health poll:
   - Watch the deployment rollout status via the Kubernetes API (or flux events)
   - Report status back to forest via the existing release status mechanism
   - Timeout after a configurable duration (default 5m)
3. The `forest release create` command would show: "Manifests applied. Waiting for rollout... Deployment healthy." or "Manifests applied. Rollout failed: CrashLoopBackOff after 5m."

### Option B: `wait: true` with retry-on-failure

1. Keep `wait: true` but add `retryInterval: 30s` to the Kustomization CR
2. On health check failure, flux retries with the latest source revision
3. This relies on flux behavior — need to verify that flux re-applies manifests on retry even when the previous apply succeeded

### Option C: Configurable per-project

Add a `wait` field to the flux destination metadata:

```cue
destinations: [{
    destination: "flux-dev.*"
    type: _destinationTypes.flux
    metadata: {
        wait: false  // or "true", "timeout:60s"
    }
}]
```

## Files to change

- `crates/forest-runner/src/destinations/fluxv1.rs` — `generate_kustomization_cr()` function (line ~362)
- `crates/forest-server/src/destinations/fluxv1.rs` — add post-release health monitoring
- `crates/forest-runner/src/destinations/fluxv1.rs` — add metadata field for `wait` configuration

## Testing

- Deploy an app with a broken config (missing env var)
- Run `forest release create` — verify manifests are applied
- Fix the config and run `forest release create` again — verify the fix lands without manual intervention
- Verify health status is reported back to the user
