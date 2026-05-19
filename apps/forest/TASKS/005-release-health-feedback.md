# 005: Add post-release health monitoring and feedback

## Problem

`forest release create` returns "Release completed successfully" as soon as manifests are committed to the flux git repo. There is no feedback about whether the deployment actually started, became healthy, or is crash-looping.

The developer has to manually run `kubectl logs` or `kubectl get pods` to discover failures. This makes the release workflow feel unreliable — "success" doesn't mean success.

## Desired behavior

After a successful release, `forest release create` should optionally monitor the deployment and report:

```
==> step 1/3: prepare
==> step 2/3: annotate
==> step 3/3: release
Release completed for destination: flux-dev/home/001

Watching rollout...
  Deployment/dev/forest: 0/1 ready (waiting for container)
  Deployment/dev/forest: 0/1 ready (CrashLoopBackOff)
  Deployment/dev/forest: 1/1 ready ✓

Release healthy.
```

Or on failure:

```
Watching rollout...
  Deployment/dev/forest: 0/1 ready (CrashLoopBackOff for 2m)

Release unhealthy. Latest logs:
  Error: S3_ACCESS_KEY not set
```

## Implementation plan

### Phase 1: `--watch` flag on `forest release create`

Add a `--watch` flag (or `--no-watch` to opt out) that:

1. After the release step completes, connect to the target cluster (via the flux destination's kubeconfig or a configured context)
2. Watch the deployment's rollout status
3. Stream status updates to stderr
4. Timeout after a configurable duration (default 5m)
5. On failure, fetch and display the last N lines of container logs

This requires the forest CLI to have kube credentials, which it may not have for remote clusters. So this should be best-effort — if kube access isn't available, just print a reminder about how to check manually.

### Phase 2: Release status webhook from flux

Leverage the existing flux notification/webhook infrastructure:

1. The flux Kustomization CR already supports Alert resources that fire on reconciliation events
2. The forest release system can listen for these webhooks (it already has `reconcile_url` and `webhook_secret` metadata)
3. When a webhook arrives indicating reconciliation success/failure, update the release status in forest

### Phase 3: Forest release status API

Expose the release health status via the forest gRPC API so that `forest release create --watch` can stream status from the forest server rather than needing direct kube access.

## Files to change

### Phase 1
- `crates/forest/src/cli/release/create.rs` — add `--watch` flag
- `crates/forest/src/cli/release/watch.rs` (new) — deployment rollout watcher
- `crates/forest-runner/src/destinations/fluxv1.rs` — expose cluster connection info

### Phase 2
- `crates/forest-server/src/webhooks.rs` — handle flux reconciliation events
- `crates/forest-server/src/destinations/fluxv1.rs` — update release status on webhook

## Testing

- Release a healthy app with `--watch` — verify it reports "healthy"
- Release a broken app with `--watch` — verify it reports the error and shows logs
- Release without `--watch` — verify existing behavior is unchanged
