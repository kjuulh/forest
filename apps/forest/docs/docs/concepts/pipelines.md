# Pipelines

A pipeline is a project's rollout as a DAG of stages: deploy to dev, wait or plan, then deploy to prod. Releasing through a pipeline is the normal path. It keeps the environment order, the plan approvals and the [policies](policies.md) the project configured, where a direct `--env prod` release skips the ordering and the approvals.

## Defining stages

Stages are a JSON **object keyed by stage name**:

```bash
forest project pipeline create -o my-org -p my-service --name rollout --stages-json '{
  "deploy-dev":  {"type": "deploy", "environment": "dev"},
  "plan-prod":   {"type": "plan",   "environment": "prod", "depends_on": ["deploy-dev"]},
  "deploy-prod": {"type": "deploy", "environment": "prod", "depends_on": ["plan-prod"]}
}'
```

`--stages-file <path>` reads the same JSON from a file. A JSON array is refused (`invalid type: sequence, expected a map`).

| Type | Fields |
|---|---|
| `deploy` | `environment` |
| `plan` | `environment`, `auto_approve` (default `false`), `destroy` (default `false`) |
| `wait` | `duration_seconds` |
| `gate` | `requires: [{signal, in: [...]}]`, `timeout_seconds`, `on_timeout: "fail" \| "proceed"` |

`depends_on` builds the DAG, and stages without dependencies start immediately. Cycles, unknown dependencies, a gate without requirements or timeout, and a `destroy` plan with `auto_approve` are all refused.

## Stage lifecycle

```
PENDING → ACTIVE → SUCCEEDED
                 → FAILED      (dependent stages are CANCELLED)
                 → SUPERSEDED
plan:     ACTIVE → AWAITING_APPROVAL → SUCCEEDED | FAILED (rejected)
```

On a server with the v0.3.17 [policy enforcement changes](policies.md), a stage for an environment with a branch restriction **fails** for a disallowed branch before a plan is offered for approval, and an unmet same-artifact deploy soak waits and is re-checked. Released v0.3.16 servers do not enforce pipeline branch restrictions and can count plans toward soak; confirm the server revision before relying on these checks.

## Plan stages and approval

With `auto_approve: false`, a plan stage parks as `AWAITING_APPROVAL` once the plan completes. `forest release show <slug>` prints the plan output and the command to approve it:

```bash
forest release approve <slug> -o my-org -p my-service [--stage plan-prod]
forest release reject  <slug> -o my-org -p my-service -r "<what to change>"
```

For `forest/terraform@1` destinations, what you approve is what gets applied. The plan stage saves the binary plan with the provider lock file, and the deploy stage of the same release applies **that file** without planning again. If the state changed in between, terraform refuses the stale plan and the deploy fails; re-run the release and review the new plan. Saved plans are deleted once their apply is attempted, and pruned after 7 days if never applied.

So that one release's apply doesn't make another's saved plan stale, a plan stage for a `forest/terraform@1` destination waits while another release of the same project holds that destination:

- its plan for the destination finished, the saved plan is under 7 days old, and its deploy stage has not finished yet (still pending behind other stages, queued or running); or
- its plan for the destination was queued earlier and is still queued or running.

The waiting plan step stays `QUEUED` and its log shows `forest: waiting for release <slug> to apply its plan for <destination> before planning`. It plans against fresh state once that deploy finishes, or once the other plan is rejected, cancelled (for example because an upstream deploy failed), superseded, discarded, or older than 7 days. A plan still awaiting approval holds later plans for the same destination until someone approves or rejects it. Deploy stages, releases without a plan stage, and destination types that save no plan never wait for this. (Server behaviour after v0.3.18.)

A plan stage with `"destroy": true` plans the removal of everything the project manages at that environment's terraform destinations, and its output starts with a `DESTROY PLAN` banner. It can never be auto-approved. Resources with `prevent_destroy` make it fail, which is the intended outcome for anything holding data.

## Starting a pipeline

- **From an annotation (preferred):** a trigger with `--use-pipeline` ([triggers](triggers.md)), for example `--branch '^main$' --source-type '^ci$' --use-pipeline`. CI then only annotates: `forest release prepare` + `forest release annotate --source-type ci`, or `forest release create --source-type ci` without a target.
- **Explicitly:** `forest release release <slug> -e <env> --pipeline` or `forest release create -e <env> --pipeline`.

The no-target `create` shorthand is in v0.3.17 and newer. On released v0.3.16 use `prepare` + `annotate`; `create` requires an environment and skips triggers. Check `create --help`, not a local build's version string.

Either way the **whole pipeline** runs: `-e` and the trigger's environments do not limit which stages run. When a project has several pipelines, the first enabled one by name is used.

## CLI commands

```bash
forest project pipeline create -o <org> -p <project> --name <name> --stages-json '<json>' | --stages-file <path>
forest project pipeline list   -o <org> -p <project>
forest project pipeline update -o <org> -p <project> --name <name> --stages-json '<json>'
forest project pipeline delete -o <org> -p <project> --name <name>
```
