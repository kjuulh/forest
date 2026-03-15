# Plan Stage Implementation

## What it is

A new pipeline stage type `Plan` that runs a destination's dry-run mode (e.g. `terraform plan`) before the actual deploy, with an optional approval gate between plan and deploy.

## Changes Made

### Database
- **Migration `20260315000000_plan_stage.sql`**: Adds `mode TEXT NOT NULL DEFAULT 'deploy'` and `plan_output TEXT` columns to `release_states`. Recreates partial unique index `idx_release_active` scoped by `(project_id, destination_id, mode)`.

### Domain Model (`services/release_pipeline.rs`)
- `StageConfig::Plan { environment: String, auto_approve: bool }` — new variant
- `StageType::Plan` — new discriminator
- `ApprovalStatus` enum: `AwaitingApproval | Approved | Rejected`
- `StageState` gains: `approval_status`, `approval_at`, `approved_by` (all optional, serde-skipped when None)
- 5 new unit tests: serde roundtrip, auto_approve default, plan→deploy DAG flow, approval status serde

### DestinationEdge Trait (`destinations.rs`)
- `plan()` method — default returns `Ok(None)`, override to return plan output
- `supports_plan()` method — default `false`
- `DestinationService` wrappers for both

### Terraform (`destinations/terraformv1.rs`)
- Implements `plan()` using existing `Mode::Prepare` (terraform plan)
- `supports_plan() -> true`

### Proto
- **`release_pipelines.proto`**: `STAGE_TYPE_PLAN = 3`, `PlanStageConfig { environment, auto_approve }`, `plan = 12` in oneof, `PIPELINE_STAGE_STATUS_AWAITING_APPROVAL = 6`
- **`releases.proto`**: `prepare_only = 6` on `ReleaseRequest`, `approval_status = 9` on `PipelineStageUpdate`, `PIPELINE_RUN_STAGE_TYPE_PLAN = 3`, `PIPELINE_RUN_STAGE_STATUS_AWAITING_APPROVAL = 6`, approval fields on `PipelineStageState` and `PipelineRunStage`, new RPCs: `ApprovePlanStage`, `RejectPlanStage`, `GetPlanOutput`

### IntentCoordinator (`intent_coordinator.rs`)
- **Step 3a (derive ACTIVE status)**: Plan arm checks child release_states. If all succeeded: auto_approve → Succeeded, else sets `AwaitingApproval` and waits. Handles `Approved` → Succeeded, `Rejected` → Failed.
- **Step 3c (activate PENDING)**: Plan arm creates child `release_states` with `mode = 'plan'` (identical to Deploy except mode column).

### Scheduler (`scheduler.rs`)
- Reads `release_state.mode` after fetching state
- Branches: `"plan"` → `prepare()` + `plan()` + store output; `"deploy"` → `prepare()` + `release()`

### gRPC (`grpc/release.rs`)
- `approve_plan_stage`: Loads intent FOR UPDATE, verifies Plan + AwaitingApproval, sets Approved, nudges coordinator
- `reject_plan_stage`: Same but sets Rejected
- `get_plan_output`: Queries child release_states for plan_output text
- Updated all stage proto conversions to include Plan type + approval fields

### Release Event Store (`services/release_event_store.rs`)
- `ReleaseState` struct gains `mode: String`, `plan_output: Option<String>`
- `get_release_state()` query updated to SELECT new columns

### CLI (`forest/src/cli/project/pipeline.rs`)
- `JsonStageConfig::Plan { environment, auto_approve }` variant
- `parse_stages_from_json` and `format_stages` handle Plan

### Other touched files
- `forest/src/grpc.rs`: Added `prepare_only: false` to existing `ReleaseRequest` construction
- `tests/accepttest/fixtures/when.rs`: Added `prepare_only: false` to test `ReleaseRequest`
- `grpc/release_pipelines.rs`: Proto conversion for Plan stage config

### Bug fix (pre-existing)
- `services/release_registry.rs`: `get_release_annotation_by_project()` was returning empty `destinations`. Fixed to join `release_states` + `destinations` to populate destination info on artifacts. This fixed the failing `test_full_release_flow` acceptance test.

## Test Results
- 57 unit tests pass (including 5 new Plan stage tests)
- 9 acceptance tests pass (including previously-broken `test_full_release_flow`)

## Not Yet Done
- `--prepare` CLI flag on release command (synthetic plan-only pipeline)
- `forest release approve/reject/plan-output` CLI subcommands
- Acceptance test for full plan→approve→deploy flow (needs in-process plan execution wired up)
- Remote runner plan mode support (WorkAssignment.mode field in proto defined but not wired in runner)
