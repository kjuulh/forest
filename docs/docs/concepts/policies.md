# Policies

Policies are guardrails that gate releases to specific environments. They enforce rules like soak times, branch restrictions, and approval requirements before a release is allowed to proceed.

## Policy Types

### Soak Time

Requires that a release has been successfully deployed to a **source environment** for a minimum duration before it can be released to the **target environment**.

```bash
forest project policy create staging-soak \
  --organisation my-org \
  --project my-service \
  --type soak_time \
  --source-environment staging \
  --target-environment prod \
  --duration 7200  # 2 hours in seconds
```

This means: "Before releasing to `prod`, the same artifact must have been running in `staging` for at least 2 hours."

The soak time check queries the `release_states` table for the last successful deployment to the source environment and compares the elapsed time.

### Branch Restriction

Restricts which branches can be released to a target environment:

```bash
forest project policy create prod-branch \
  --organisation my-org \
  --project my-service \
  --type branch_restriction \
  --target-environment prod \
  --branch "^main$"
```

This means: "Only releases from the `main` branch can go to `prod`."

Branch restrictions are enforced at the gRPC layer — the release is rejected immediately if the branch doesn't match.

### External Approval

Requires a minimum number of approvals from external systems before a release can proceed:

```bash
forest project policy create prod-approval \
  --organisation my-org \
  --project my-service \
  --type external_approval \
  --target-environment prod \
  --required-approvals 2
```

External systems approve or reject via the gRPC API:

```
ExternalApproveRelease { intent_id, approver }
ExternalRejectRelease { intent_id, reason }
```

## Evaluation

You can dry-run policy evaluation without creating a release:

```bash
forest project policy evaluate \
  --organisation my-org \
  --project my-service \
  --environment prod
```

This returns per-policy pass/fail results with human-readable reasons:

```
soak_time (staging-soak): PASS — deployed to staging 3h ago (required: 2h)
branch_restriction (prod-branch): PASS — branch "main" matches "^main$"
external_approval (prod-approval): FAIL — 1/2 approvals received
```

## Enforcement Points

Different policy types are enforced at different points:

| Policy | Enforced At |
|--------|------------|
| Branch restriction | gRPC `release()` handler — immediate rejection |
| Soak time | IntentCoordinator — deferred, re-evaluated periodically |
| External approval | IntentCoordinator — waits for approval signals |

## CLI Commands

```bash
forest project policy create <name> --type <type> [options]
forest project policy list --organisation my-org --project my-service
forest project policy update <name> [options]
forest project policy delete <name>
forest project policy evaluate --organisation my-org --project my-service --environment prod
```

## Interaction with Triggers

Triggers create releases, but policies still apply. A triggered release to `prod` will be blocked by a soak time policy if the staging deployment is too recent — unless the trigger has `force_release` enabled.
