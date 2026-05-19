# Approval Gate — Implementation Log

## Overview

New policy type `POLICY_TYPE_EXTERNAL_APPROVAL` that requires human approval before a release can deploy to a target environment.

**Rules:**
- Scoped to a single release intent + target environment
- Release author cannot approve (unless admin → red "Bypass" button)
- All org members can approve
- Rejection is a vote, not a permanent block
- No timer retry — NATS signal on decision triggers re-evaluation

---

## Forage (client) Changes — DONE

### Proto

**New file:** `interface/proto/forest/v1/policies.proto`
- `POLICY_TYPE_EXTERNAL_APPROVAL = 3` added to `PolicyType` enum
- `ApprovalConfig { target_environment, required_approvals }` message
- `ApprovalState { required_approvals, current_approvals, decisions }` message
- `ApprovalDecisionEntry { user_id, username, decision, decided_at, comment }` message
- `PolicyEvaluation` extended with `optional ApprovalState approval_state = 10`
- `EvaluatePoliciesRequest` extended with `optional string release_intent_id = 4`
- `Policy`, `CreatePolicyRequest`, `UpdatePolicyRequest` oneofs extended with `ApprovalConfig approval = 12`
- New RPCs: `ApproveRelease`, `RejectRelease`, `GetApprovalState` with request/response messages

**New file:** `scripts/sync-protos.sh`
- Copies all `.proto` files from forest repo to forage, runs `buf generate`

**Regenerated:** `crates/forage-grpc/src/grpc/forest/v1/forest.v1.rs` and `forest.v1.tonic.rs`

### Domain Model

**File:** `crates/forage-core/src/platform/mod.rs`
- `PolicyConfig::Approval { target_environment, required_approvals }` variant
- `ApprovalState` struct
- `ApprovalDecisionEntry` struct
- `PolicyEvaluation.approval_state: Option<ApprovalState>` field
- New `ForestPlatform` trait methods: `evaluate_policies`, `approve_release`, `reject_release`, `get_approval_state`

### gRPC Client

**File:** `crates/forage-server/src/forest_client.rs`
- `convert_policy`: handles `policy::Config::Approval` → `PolicyConfig::Approval`
- `policy_config_to_grpc`: handles `PolicyConfig::Approval` → gRPC
- `convert_policy_evaluation`: maps policy type 3 → "approval", maps `approval_state`
- `convert_approval_state`: maps gRPC `ApprovalState` → domain
- `evaluate_policies` impl: calls `PolicyServiceClient::evaluate_policies` with `release_intent_id`
- `approve_release` impl: calls `PolicyServiceClient::approve_release`
- `reject_release` impl: calls `PolicyServiceClient::reject_release`
- `get_approval_state` impl: calls `PolicyServiceClient::get_approval_state`
- Fixed `PipelineStage::Plan` match arm (new variant from forest proto sync)
- Fixed `ReleaseRequest` missing `prepare_only` field (new field from forest proto sync)

### Test Support

**File:** `crates/forage-server/src/test_support.rs`
- `MockPlatformClient`: default impls for `evaluate_policies`, `approve_release`, `reject_release`, `get_approval_state`

### Routes

**File:** `crates/forage-server/src/routes/platform.rs`

**New routes:**
- `POST /orgs/{org}/projects/{project}/releases/{slug}/approve` → `approve_release_submit`
- `POST /orgs/{org}/projects/{project}/releases/{slug}/reject` → `reject_release_submit`

**New handler structs:**
- `ApprovalForm { csrf_token, release_intent_id, target_environment, comment, force_bypass }`
- `CreatePolicyForm` extended with `required_approvals: Option<i32>`

**Modified handlers:**
- `create_policy_submit`: handles `policy_type = "approval"` with validation
- `policies_page`: maps `PolicyConfig::Approval` to template context
- `edit_policy_page`: maps `PolicyConfig::Approval` to template context
- `artifact_detail`: fetches policy evaluations per environment, passes `policy_evaluations`, `release_intent_id`, `is_release_author`, `is_admin` to template

### Templates

**File:** `templates/pages/policies.html.jinja`
- Policy list: "Approval Required" badge with target env + approval count
- Create form: "Approval Required" option in type dropdown
- Approval fields: target environment select + required approvals number input
- JavaScript: toggles visibility of soak/branch/approval field sets

**File:** `templates/pages/artifact_detail.html.jinja`
- New "Policy Requirements" section between Pipeline and Destinations
- Shows all policy evaluations (soak, branch, approval) with pass/fail icons
- Approval UI:
  - Approval count badge (current/required)
  - Decision history (username, approved/rejected, comment)
  - **Approve** button (green) — shown to non-authors
  - **Bypass (Admin)** button (red) — shown to admin authors with confirmation dialog
  - **Reject** button (red outline) — shown to all eligible members
  - "You cannot approve your own release" message for non-admin authors

---

## Forest (core) Changes — NEEDS MANUAL APPLICATION

### Proto
**File:** `interface/proto/forest/v1/policies.proto` — same changes as forage copy above

### DB Migration
**New file:** `crates/forest-server/migrations/20260315000001_approval_decisions.sql`
```sql
CREATE TABLE approval_decisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL REFERENCES release_intents(id) ON DELETE CASCADE,
    policy_id UUID NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    target_environment TEXT NOT NULL,
    user_id UUID NOT NULL,
    username TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('approved', 'rejected')),
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- unique per user per intent per env, lookup index for counting
```

### Policy Engine
**File:** `crates/forest-server/src/services/policy.rs`
- `PolicyType::Approval`, `ApprovalConfig` struct
- `PolicyConfig::Approval(ApprovalConfig)` variant
- `ApprovalStateInfo`, `ApprovalDecisionInfo` structs
- `PolicyEvaluation.approval_state: Option<ApprovalStateInfo>`
- `evaluate_for_environment` gains `release_intent_id: Option<&Uuid>` param
- `check_approval`: queries approval_decisions, compares count vs required
- `record_approval_decision`: upserts into approval_decisions
- `get_intent_actor_id`: queries release_intents.actor_id
- `find_approval_policy_for_environment`: finds enabled approval policy for target env
- `get_approval_state`: returns current approval state for display

### Intent Coordinator
**File:** `crates/forest-server/src/intent_coordinator.rs`
- `check_approval_policies` called after `check_soak_time_policies` for deploy stages
- If blocked: logs and continues (no timer retry, NATS-triggered re-eval on decision)

### Release Event Store
**File:** `crates/forest-server/src/services/release_event_store.rs`
- `check_approval_policies(tx, project_id, release_intent_id, target_environment) -> Option<String>`
- Loads enabled approval policies, counts approved decisions, blocks if insufficient

### gRPC Handlers
**File:** `crates/forest-server/src/grpc/policies.rs`
- `record_to_grpc`: handles `PolicyConfig::Approval`
- `eval_to_grpc`: handles `PolicyType::Approval`, maps `approval_state`
- `extract_config` / `extract_update_config`: handles approval config
- `evaluate_policies`: passes `release_intent_id` through
- `approve_release`: validates actor != intent author (unless force_bypass), records decision, publishes NATS
- `reject_release`: records rejection decision
- `get_approval_state`: returns current approval state

### Caller Updates
- `src/grpc/release.rs`: `evaluate_for_environment` calls gain `None` as 4th arg
- `src/scheduler.rs`: same

---

## Verification
- Forage: **169 tests passing**, compiles clean (0 errors, 0 warnings)
- Forest: tool permissions blocked writes — all code is ready, needs to be applied from forest repo context

## Next Steps
1. Apply forest changes (run claude from the forest directory, or grant write access)
2. Run `buf generate` in forest to regenerate gRPC interface stubs
3. Run forest tests
4. E2E test: create approval policy, trigger release, verify UI shows approval buttons

---

## Plan Stage Support (Prepare-Before-Deploy)

### Overview

Added support for "plan" pipeline stages — destinations that run a prepare/dry-run (e.g. terraform plan) and require approval of the output before the actual deploy proceeds. Forest already had full infrastructure for this; this work surfaces it in the Forage UI.

### Changes

#### forage-core (`crates/forage-core/src/platform/mod.rs`)
- Added `PipelineStageConfig::Plan { environment, auto_approve }` variant
- Added `approval_status: Option<String>` and `auto_approve: Option<bool>` to `PipelineRunStageState`
- Added 3 new `ForestPlatform` trait methods: `approve_plan_stage`, `reject_plan_stage`, `get_plan_output`
- Added `PlanOutput` struct (`plan_output: String`, `status: String`)

#### forage-server gRPC client (`forest_client.rs`)
- `convert_pipeline_stage`: handles `Plan` config variant (was previously mapped to empty Deploy)
- `convert_pipeline_stage_state`: recognizes `Plan` stage type + `AwaitingApproval` status + new fields
- `convert_stages_to_grpc`: handles `PipelineStageConfig::Plan` → `PlanStageConfig`
- Implemented `approve_plan_stage`, `reject_plan_stage`, `get_plan_output` calling forest's RPCs

#### forage-server routes (`routes/platform.rs`)
- Added 3 API routes:
  - `POST /api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/approve`
  - `POST /api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/reject`
  - `GET /api/orgs/{org}/projects/{project}/plan-stages/{stage_id}/output`
- `ApiPipelineStage` now includes `approval_status` and `auto_approve`
- `build_timeline_json`: plan stages with `AWAITING_APPROVAL` status are shown with that status; releases with plan stages awaiting approval are treated as `needs_action` (not hidden)

#### Pipeline builder (`static/js/pipeline-builder.js`)
- Added "plan" as third stage type in dropdown
- Plan stage config: environment + auto-approve checkbox
- Purple color scheme for plan nodes in DAG visualization

#### Svelte timeline (`frontend/src/ReleaseTimeline.svelte`)
- `approvePlanStage(release, stage, reject)` function for approve/reject via API
- `viewPlanOutput(release, stage)` function for on-demand plan output fetching (toggle)
- Plan stages render with purple shield icon when `AWAITING_APPROVAL`
- "Approve plan" / "Reject" buttons on plan stages awaiting approval
- "View plan" / "Hide plan" button to toggle plan output display
- Plan output shown in collapsible `<pre>` block (monospace, max-height 256px with scroll)
- Summary line shows plan stage badge + approve button when plan awaiting approval

#### Status helpers (`frontend/src/lib/status.js`)
- Added `planStageLabel(status)` function
- `pipelineSummary`: detects `AWAITING_APPROVAL` plan stages → "Awaiting plan approval" (purple)

#### Slack notifications (`forage-core/src/integrations/router.rs`)
- Plan stage rendering in Slack blocks: "Planning", "Awaiting plan approval", "Plan approved", "Plan failed"
- Shield emoji for AWAITING_APPROVAL status

#### Test support (`test_support.rs`)
- Added default mock implementations for the 3 new trait methods

### Forest Runner Infrastructure

#### Proto (`runner.proto`)
- Added `ReleaseMode` enum: `RELEASE_MODE_UNSPECIFIED`, `RELEASE_MODE_DEPLOY`, `RELEASE_MODE_PLAN`
- Added `mode` field (type `ReleaseMode`) to `WorkAssignment` — tells remote runners whether to deploy or plan
- Added `plan_output` field (optional string) to `CompleteReleaseRequest` — runners send plan output back

#### Scheduler (`scheduler.rs`)
- Reads `release_state.mode` and maps to `ReleaseMode::Plan` / `ReleaseMode::Deploy`
- Includes `mode` in `WorkAssignment` when dispatching to remote runners

#### Runner gRPC handler (`grpc/runner.rs`)
- `complete_release`: stores `plan_output` from `CompleteReleaseRequest` to `release_states.plan_output` in DB

#### Terraform destination (`destinations/terraformv1.rs`)
- `plan()`: now captures actual terraform plan stdout (not just a marker)
- Added `run_capture()` method — same as `run()` but captures stdout into a String
- Added `run_command_capture()` — like `run_command()` but returns captured stdout while still logging

#### Runner crate (`forest-runner`)
- `RunnerDestination` trait: added `plan()` method (default returns None)
- `Executor`: checks `ReleaseMode` from `WorkAssignment`, calls `plan()` instead of `release()` for plan mode
- `RunnerSession::complete_release`: accepts optional `plan_output` parameter
- `run_destination_plan()` function: prepare + plan, returns `Option<String>`
