# forest/deployment

The deployment-hook contract. Any component that wants to be a
*destination type* — i.e. something forest can deploy releases to —
imports this module and implements its hooks.

## What it defines

The `ForestDeploymentHookHandler` trait (and the corresponding CUE
shapes) covers the full release lifecycle:

- **`prepare`** — gather inputs, render manifests, no side effects on
  the target yet. The `plan` stage in a pipeline consumes this output.
- **`release`** — actually apply the change. Idempotent: the same
  release should converge to the same end state.
- **`status`** — health check the target. Used by the scheduler to
  decide when a release has succeeded or failed.

