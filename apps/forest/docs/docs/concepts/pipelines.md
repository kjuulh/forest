# Pipelines

Pipelines orchestrate multi-stage deployments as directed acyclic graphs (DAGs). They let you model complex rollout strategies — deploy to staging, wait for soak time, run a plan, then deploy to production.

## Stages

A pipeline consists of stages with three types:

### Deploy Stage

Deploys to a specific environment's destinations:

```json
{
  "name": "deploy-staging",
  "deploy": {
    "environment": "staging"
  }
}
```

### Wait Stage

Pauses for a specified duration:

```json
{
  "name": "soak-2h",
  "wait": {
    "duration_seconds": 7200
  }
}
```

### Plan Stage

Runs a dry-run / preview (e.g., `terraform plan`) before deploying:

```json
{
  "name": "plan-prod",
  "plan": {
    "environment": "prod",
    "auto_approve": false
  }
}
```

When `auto_approve` is `false`, the stage enters an **AwaitingApproval** sub-state after the plan completes, requiring manual approval before proceeding.

For `forest/terraform@1` destinations, what you approve is what gets applied. The plan stage saves the binary plan (`terraform plan -out`) together with the provider lock file. The deploy stage of the same release then applies **that file**, and doesn't plan again. If the state changed between approval and apply (someone else applied), terraform refuses the plan as stale and the deploy fails. Re-run the release and review the new plan. A deploy stage whose plan stage ran but has no saved plan also fails rather than re-planning. Saved plans are deleted once their apply has been attempted, and pruned after 7 days if never applied.

### Destroying

A plan stage with `"destroy": true` plans the removal of everything the project manages at that environment's `forest/terraform@1` destinations (`terraform plan -destroy`). The deploy stage after it applies that saved plan, exactly as it applies any other.

```json
[
  {"name": "destroy-dev", "plan": {"environment": "dev", "destroy": true}},
  {"name": "apply-dev", "deploy": {"environment": "dev"}, "depends_on": ["destroy-dev"]}
]
```

A destroy plan stage can never set `auto_approve`; the pipeline is refused. Its plan output starts with a `DESTROY PLAN` banner. It destroys what the state holds, so resources with `prevent_destroy` make the plan fail. That's the intended outcome for anything holding data: remove the guard deliberately first.

## DAG Structure

Stages declare dependencies via `depends_on`, forming a DAG:

```json
[
  {"name": "deploy-staging", "deploy": {"environment": "staging"}},
  {"name": "soak", "wait": {"duration_seconds": 7200}, "depends_on": ["deploy-staging"]},
  {"name": "plan-prod", "plan": {"environment": "prod", "auto_approve": false}, "depends_on": ["soak"]},
  {"name": "deploy-prod", "deploy": {"environment": "prod"}, "depends_on": ["plan-prod"]}
]
```

This creates a linear pipeline:

```
deploy-staging → soak (2h) → plan-prod → deploy-prod
```

Stages without dependencies (root stages) start immediately. Forest validates the DAG using topological sort — cycles are rejected.

## Stage Lifecycle

Every stage follows a consistent lifecycle:

```
PENDING → ACTIVE → SUCCEEDED
                 → FAILED
                 → CANCELLED
```

Each stage tracks `queued_at`, `started_at`, and `completed_at` timestamps.

### Plan Stage Approval

Plan stages have an additional sub-state:

```
PENDING → ACTIVE → AwaitingApproval → Approved → SUCCEEDED
                                    → Rejected → FAILED
```

Approve or reject:

```bash
# Via gRPC (CLI support planned)
ApprovePlanStage { intent_id, stage_name }
RejectPlanStage { intent_id, stage_name }
```

## IntentCoordinator

The **IntentCoordinator** is Forest's saga orchestrator. It:

1. Watches for pipeline release intents (via NATS + 5s polling fallback)
2. Evaluates stage readiness based on the `stage_states` DAG
3. Activates stages when all dependencies are satisfied
4. Handles transitive cancellation (if a stage fails, dependent stages are cancelled)
5. Marks the intent as succeeded when all stages complete

The coordinator is idempotent — it can safely re-evaluate at any time.

## Creating Pipelines

```bash
forest project pipeline create prod-rollout \
  --organisation my-org \
  --project my-service \
  --stages-json '[
    {"name": "deploy-staging", "deploy": {"environment": "staging"}},
    {"name": "soak", "wait": {"duration_seconds": 7200}, "depends_on": ["deploy-staging"]},
    {"name": "deploy-prod", "deploy": {"environment": "prod"}, "depends_on": ["soak"]}
  ]'
```

Or from a file:

```bash
forest project pipeline create prod-rollout \
  --stages-file pipeline.json
```

## CLI Commands

```bash
forest project pipeline create <name> --stages-json '<json>' | --stages-file <path>
forest project pipeline list --organisation my-org --project my-service
forest project pipeline update <name> --stages-json '<json>'
forest project pipeline delete <name>
```

## Using Pipelines

Reference pipelines when releasing:

```bash
forest release release --environment prod --pipeline
```

Or configure [triggers](triggers.md) with `use_pipeline: true` for automatic pipeline execution.
