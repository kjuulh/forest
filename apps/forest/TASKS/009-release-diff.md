# 009: Add `forest release diff` to show changes between releases

## Problem

When debugging a broken deployment, there's no way to see what changed between the current and previous release manifests. You have to manually clone the flux git repo and diff the files.

## Desired behavior

```bash
$ forest release diff --env dev
Comparing: rawpotion-forest dev (current vs previous)

--- 30-deployment.yaml
+++ 30-deployment.yaml
@@ -42,6 +42,10 @@
           envFrom:
             - secretRef:
                 name: forest-secrets
+            - secretRef:
+                name: forest-db-credentials
+            - secretRef:
+                name: forest-s3-s3-credentials

--- 25-forage-nats.yaml (new)
+++ 25-forage-nats.yaml
+apiVersion: forage.rawpotion.io/v1alpha1
+kind: NatsUser
...
```

## Implementation plan

1. The forest server already stores artifact files per release. Add an API endpoint that returns the manifest diff between two artifact versions.
2. The `forest release diff` CLI command calls this API and renders the diff.
3. Optionally support `forest release diff <slug-a> <slug-b>` for comparing arbitrary releases.

## Files to change

- `crates/forest/src/cli/release/diff.rs` (new) — CLI command
- `crates/forest/src/cli/release/mod.rs` — register subcommand
- `crates/forest-server/src/grpc/release.rs` — add diff endpoint (or compute client-side from artifacts)

## Nice-to-have

- Show the diff in the `forest release create` output when `--verbose` is set
- Color-coded diff output (green for additions, red for removals)
