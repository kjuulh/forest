# Migrate Policy to Event-Sourced Aggregate

## Status: Done

## Why
Policy changes are security-critical. A full event log of when soak_time durations, branch restrictions, or external approval requirements were changed provides compliance audit trail. Currently simple CRUD on `policies` table with no history.

## Approach
Same pattern as destination/trigger aggregates using `forest-event-store`.

### Events
- `PolicyCreated { policy_id, project_id, name, policy_type, config: serde_json::Value }`
- `PolicyConfigUpdated { config: serde_json::Value }`
- `PolicyEnabledToggled { enabled: bool }`
- `PolicyDeleted`

### Complexity: Medium
- 3 config variants (SoakTime, BranchRestriction, ExternalApproval) — keep as JSONB Value in events, typed deserialization stays in projection/evaluation
- `evaluate_for_environment()` remains a projection read — no change to evaluation logic
- ExternalApproval has its own `approval_decisions` table — that stays separate (it's a different lifecycle)

### Files
- Create: `domains/policy.rs`, `services/policy_aggregate.rs`
- Edit: `domains/mod.rs`, `services/mod.rs`, `grpc/policies.rs`
- Delete: write methods from `services/policy.rs`

### Depends on
- Trigger aggregate migration (establishes the full pattern with project-scoped stream keys)
