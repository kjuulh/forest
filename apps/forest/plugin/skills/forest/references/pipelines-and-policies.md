# Pipelines, triggers, approval and policies

## Prefer pipelines

Release by **annotating** and let an anchored trigger start the pipeline. The pipeline encodes dev-before-prod ordering, plan approval and destroy safety. Policies are enforced by the **server**, so what you can rely on depends on the server's version, not your CLI's. Servers running the v0.3.17 change check branch restriction on pipeline stages and `-d` releases, and count only this artifact's dev deploys toward soak. Older servers do neither (see the table below). Unless you know the server has the change, keep the branch check in CI and confirm same-artifact dev exposure at approval. A direct `release -e prod` skips ordering and plan approval; never use it or `-d` to recover from a policy denial.

| Situation | Do this |
|---|---|
| Normal change (CI or local) | `forest release prepare` then `forest release annotate --context-title "<t>" --source-type ci`. The matching trigger runs the pipeline. Where `release create --help` shows `--environment` as optional, `forest release create --source-type ci` does the same in one command. |
| CI on GitHub Actions | `understory-io/forest/.github/actions/release` with **no** `environment`, or `service-release.yml`; both annotate without a direct release call, but triggers can deploy |
| No trigger matches, but the pipeline should run | `forest release create -e <env> --pipeline` (or `forest release release <slug> -e <env> --pipeline`) |
| User asked for exactly one environment | `forest release create -e <env>`. Say that it bypasses the pipeline. |

Version boundary: v0.3.16 and older have the old `create` behaviour; the new routing is in v0.3.17 and newer. A build from source may print an older number, so check `forest release create --help` for an optional `--environment`.

- **Old `create`:** `--environment` is required; annotation skips triggers. `--skip-commit` releases nothing (`show` reports no release intent). Use `prepare` + `annotate` for pipelines.
- **New `create`:** without `-e`/`-d`, or with `--skip-commit`, it prepares and annotates **with triggers**. It makes no direct release call, but a trigger can start the whole pipeline and deploy prod. Explicit `-e`/`-d` without `--skip-commit` preserves direct release with triggers skipped. `--pipeline`/`--force`/`--no-wait`/`--no-health` need a target; they are not trigger controls.
- There is no `--no-release`. To upload without firing triggers, use `prepare` + `annotate --annotation-only`; this does not disable other actors later releasing the annotation.
- **`--pipeline` ignores `-e` and `-d`.** It starts the project's whole pipeline: the first enabled pipeline by name, so there is no way to choose one.
- Existing CLI limitation: `release release --pipeline` still selects or prompts for an environment before starting the pipeline, even though the pipeline ignores that filter. Pass `-e <env>` in noninteractive use; it avoids the prompt, not prod stages.
- **Don't do both.** Annotating in CI (which fires the trigger) *and* passing an `environment` to the release action double-fires or races.

## Inspect before changing anything

```bash
forest project trigger list  -o <org> -p <project>
forest project pipeline list -o <org> -p <project>
forest project policy list   -o <org> -p <project>
forest project policy evaluate -o <org> -p <project> --target-environment prod --branch <branch>
```

Read what they actually do, not their names:

- **A trigger with `--use-pipeline` ignores its own `--env`/`--dest` and runs every stage of the pipeline.** `trigger list` still shows `environments: dev`. A trigger named `deploy-dev` therefore reaches prod if the pipeline has a prod stage. This was reproduced: it ran `deploy-dev`, parked `plan-prod`, and deployed prod after approval.
- **Every regex is unanchored.** Trigger patterns (`--branch`, `--title`, `--author`, `--commit-message`, `--source-type`) and policy `--branch-pattern` are tested with "contains". `--branch main` fires for `feature/maintenance`; write `^main$`. A trigger pattern whose field is missing on the annotation does not match.
- **`policy evaluate` is a partial diagnostic, not release approval.** It has no artifact selector: soak describes the latest successful deploy of **any artifact**, not the artifact you intend to release. A PASS does not prove that artifact has soaked, and an unrelated newer deploy can produce a FAIL. It also exits 0 with `[FAIL]` lines, so neither its exit status nor a PASS is an automation gate. The server evaluates the actual artifact when releasing it.

## Pipelines

Stages are a JSON **object keyed by stage name**. The array form shown in older concept docs is refused (`invalid type: sequence, expected a map`).

```bash
forest project pipeline create -o <org> -p <project> --name rollout --stages-json '{
  "deploy-dev":  {"type": "deploy", "environment": "dev"},
  "plan-prod":   {"type": "plan",   "environment": "prod", "depends_on": ["deploy-dev"]},
  "deploy-prod": {"type": "deploy", "environment": "prod", "depends_on": ["plan-prod"]}
}'
```

| Type | Fields |
|---|---|
| `deploy` | `environment` |
| `plan` | `environment`, `auto_approve` (default false: parks for approval), `destroy` (default false; can't be combined with `auto_approve`) |
| `wait` | `duration_seconds` |
| `gate` | `requires: [{signal, in: [...]}]`, `timeout_seconds`, `on_timeout: "fail" \| "proceed"` |

`depends_on` builds a DAG. Cycles and unknown dependencies are refused, and a failed stage cancels what depends on it. `--stages-file` reads the same JSON from a file, and `pipeline update` and `pipeline delete` exist. For `forest/terraform@1`, the deploy stage applies exactly the plan that was approved. On the Gitea fork server after dc877a6, a `forest/generic@1` destination with a plan-sealing provider (grund) waits like terraform, so a plan never overlaps another release's deploy. A deploy the provider reports as a stale plan is superseded when a later active (or already deployed) release targets the destination, and otherwise replans once. A burst still deploys every release in turn unless a `supersede_pending` policy covers the environment. At dispatch, an older generic deploy is superseded when a newer release is already deployed or queued for the destination, so a soak backlog never lands older artifacts over newer ones. A rollback is a new release of the older artifact. See [pipelines](../../../../docs/docs/concepts/pipelines.md).

## Approval

A plan stage with `auto_approve: false` parks as `AWAITING_APPROVAL`. `release create`/`release` and `release show` print the exact command and exit 0; they don't wait for a human.

```bash
forest release show <slug> -o <org> -p <project>        # read the plan output first
forest release approve <slug> -o <org> -p <project> [--stage plan-prod]
forest release reject  <slug> -o <org> -p <project> -r "<what to change>"
```

Approving changes real infrastructure, so it is the user's decision. Show them the plan; never approve on your own initiative. After approval the coordinator activates the next stage on its next sweep; follow with `forest release show <slug> --follow`.

## Triggers

```bash
forest project trigger create -o <org> -p <project> --name deploy-main-via-pipeline \
  --branch '^main$' --source-type '^ci$' --use-pipeline
```

Flags: `--name`, `--branch`, `--title`, `--author`, `--commit-message`, `--source-type` (all regexes, ANDed), `-e/--env` and `-d/--dest` (repeatable; ignored with `--use-pipeline`), `--force` (cancel queued releases; it does not bypass policies), and `--use-pipeline`. Older docs show `--target-environment`, `--target-destination` and `--force-release`; those flags don't exist. Triggers fire only on `release annotate` without `--annotation-only`.

## Policies

```bash
forest project policy create -o <org> -p <project> --name prod-main-only \
  --type branch_restriction --target-environment prod --branch-pattern '^main$'
forest project policy create -o <org> -p <project> --name soak \
  --type soak_time --source-environment dev --target-environment prod --duration 3600
```

| Type | Server with the v0.3.17 change | Older server (v0.3.16 and before) |
|---|---|---|
| `branch_restriction` | `-e` and `-d` releases refused with `blocked by policy`; pipeline plan/deploy stages fail for that env; queued work rechecks its annotation branch at dispatch and fails if policy changed to disallow it | only `-e` releases checked; `-d` releases and pipeline stages are not |
| `soak_time` | source and target must differ (on the Gitea fork after dc877a6, a self-referential soak is refused; it would park every release forever); only a successful **deploy** of the **same artifact** to the source env counts; never deployed there means not soaked; direct releases wait `QUEUED`, stages wait | the scheduler counts any artifact's success; pipelines count this artifact but include plan steps; no source success at all passes |
| policy evaluation error | fails closed: release refused, trigger blocked, queued release held | treated as "no policies" |
| `supersede_pending` | newer pending releases replace older ones for the target (`--same-branch-only` to narrow); after v0.3.18 plans only replace plans and deploys only deploys, and a newer terraform plan supersedes an older plan still awaiting approval | same |
| approval (server-side; the CLI can't create it) | pipelines only | same |

Since v0.3.17, an applicable branch restriction also rejects absent, empty or whitespace-only branch metadata, even with a permissive pattern. This is a compatibility change from older servers that skipped restrictions when no branch was recorded. The server does not invent provenance from a checkout or default branch, or rewrite historical annotations; projects without an applicable restriction are unchanged.

The v0.3.17 `create` detection also leaves a detached checkout's branch unset instead of recording Git's `HEAD` sentinel. Actual CI branch metadata or an explicit source branch still takes precedence. Older `create` annotations may contain `HEAD`; inspect their recorded provenance rather than treating it as a real branch or rewriting it.

Missing same-artifact source deployment remains a `QUEUED` prerequisite, not approval and not a deadline. Since v0.3.17, `release show` and project release status expose the current artifact-specific blocker on queued steps and ready pending plan/deploy stages. A plan or another artifact's source deploy does not clear it; source success, policy changes and assignment are reflected on the next status read, without storing stale policy errors. A blocker alone is not a failed release. If evaluation fails, status reports that it cannot evaluate policies rather than implying approval. Soak measures time since the deploy succeeded, not whether the service stayed healthy, so review dev health at approval.

`--force` never bypasses a policy. A forced release behind an unmet soak simply stays `QUEUED`. Older docs show `--branch` and `--environment` on these commands, and an `external_approval` type with `--required-approvals`; the real flags are `--branch-pattern` and `--target-environment`.

## Setup and approval guardrails

1. Compare anchored destination selectors in `forest.cue` with `forest destination list -o <org>`.
2. Inspect the whole pipeline and create one anchored trigger (`--branch '^main$' --source-type '^ci$' --use-pipeline`). Trigger environment labels do not limit stages.
3. Configure branch/soak policies. On a server with the v0.3.17 change they guard every path to the environment. On an older one they are additional checks only, so retain CI's allowed-branch condition there; explicit pipeline starts bypass the trigger filter either way.
4. CI builds and pushes the immutable artifact, then `prepare` + `annotate --source-type ci --detect`, with no direct-release `environment` input.
5. Before approving prod, check the annotation's branch/SHA, the exact artifact's successful dev deploy and elapsed time, and the saved plan. Missing evidence means stop, not `--force` or `-d`. Follow the **deploy** stage and verify the running revision ([releases.md](releases.md#proving-a-deployment)).

All of these commands change shared configuration. Propose the exact commands and let the user approve them.
