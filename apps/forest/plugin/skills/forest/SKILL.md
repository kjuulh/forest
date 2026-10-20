---
name: forest
description: Use Forest CLI for forest.cue, forest.component.cue and forest.lock; project/component commands; shared tools; component or tool RC publishing; contexts/auth; and pipeline-first releases, approvals and failures. Use for deploy/ship tasks in Forest projects, not unrelated release tooling.
---

# Forest

Forest codifies project workflows as CUE and distributes shared tools. Guidance matches released **v0.3.16** (2026-10-20). Behaviour that changed after it is labelled "after v0.3.16". Users may run an older CLI, and a local build reports the last released version, so check flag help rather than the version string.

## Pick the reference

Read only the task's reference, relative to this skill's directory (not the user's working directory). Publishing a component RC also needs the consumer steps in `components.md`; releasing through approval needs both release references.

| Task | Read |
|---|---|
| Install, contexts, login, tokens, which server a command hits, where files live | [references/setup-and-context.md](references/setup-and-context.md) |
| Depend on a component, `forest run` commands, write `forest.cue` / `forest.component.cue`, `forest validate`, lockfile, upload/external/prebuilt shapes | [references/components.md](references/components.md) |
| `forest publish`, versions, CI publishing, **component prereleases** | [references/publishing.md](references/publishing.md) |
| Shared tools: `forest global add/run/update/pin/ban`, **tool prereleases**, shims, shell integration | [references/tools.md](references/tools.md) |
| Releasing: prepare/annotate/release, environments, destinations, `--set`, promotion, rollback, reading `release show` | [references/releases.md](references/releases.md) |
| Triggers, pipelines, plan approval, policies (soak, branch) | [references/pipelines-and-policies.md](references/pipelines-and-policies.md) |
| Something failed or looks wrong | [references/troubleshooting.md](references/troubleshooting.md) |

## Choose the surface

- **Project/component commands:** `forest run <cmd>` in a project with `forest.cue`; build is `forest run build`, not `forest build`.
- **Shared tools:** `forest global run <tool> -- <args>` anywhere, or its installed shim. `global` is hidden from top-level help; use `forest global --help`.
- **Producer releases:** use the pipeline-first flow below, not a tool update or component publish.
- Currently published versions, including RCs, reject overwrite; an admin unpublish permits version reuse. An exact pin selects a version, not a lifetime content identity: binary digest locks detect replacement, while warm caches are not an online revocation check. Pin project dependencies to exact published versions: that works on every CLI, while ranges only resolve after v0.3.16. CUE module imports and tool selectors have different rules in their references.
  Only registry binary dependencies get version/digest lock entries. File/template-only components have no such lock; use exact versions for reproducible selection rather than assuming a range is pinned.

## Before anything that changes shared state

These commands act immediately on whatever server the active context points at: `publish`, `release create|release|annotate|approve|reject|fail`, `project trigger|pipeline|policy create|update|delete`, `environment|destination create|update|delete`, `organisation …`, `auth token create`. An example in this skill or the docs is not permission to run one; confirm with the user.

1. **Know the target.** `forest context active` names the context and server; `FOREST_SERVER`/`--forest-server` override the server while the credentials still come from the context. Organisation and project come from `forest.cue` unless `-o`/`-p` are passed.
2. **Know what a release will actually do.** Read `forest project trigger list`, `forest project pipeline list` and `forest project policy list` for the project first. Names lie: a trigger called `deploy-dev` with `--use-pipeline` runs the *whole* pipeline, prod stages included, and trigger and policy regexes are unanchored (`main` matches `feature/maintenance`).
3. **Dry-runs are partial.** `forest publish --dry-run` needs a login; built binary preflight can execute the binary and write `.forest/component/meta.json` (prebuilt/external paths do neither). `forest release prepare` deletes and rewrites `.forest/deployment/`; `forest project policy evaluate` has no artifact context and exits 0 even when a policy fails. There is no release dry-run.
4. **Don't fetch secrets to be helpful.** `forest destination reveal`, `forest state pull|env` and `forest auth token` print credentials or state that holds them.

## Prefer pipelines over releasing straight into an environment

A normal release is an *annotation* that an anchored trigger turns into a pipeline run. Inspect the whole pipeline before annotating: `--use-pipeline` does not confine it to the trigger's environment. Pipelines preserve stage order and plan approval, but branch/soak policy gaps remain; read [references/pipelines-and-policies.md](references/pipelines-and-policies.md) before starting or approving one.

```bash
forest release prepare
forest release annotate --context-title "<what changed>" --source-type ci
```

After v0.3.16, `forest release create --source-type ci` without a target is shorthand for both steps. **It can deploy through triggers, including prod**; it is not a no-deployment mode. Check `create --help` for an optional `--environment` before using it. In CI use the `release` composite action with no `environment`, or `service-release.yml`. If no trigger matches, inspect the whole configured pipeline before requesting `create -e <env> --pipeline`; it is not limited to `<env>`. Direct environment releases require an explicit user request, never recovery from policy denial. Version-specific `--skip-commit` behavior is in [references/pipelines-and-policies.md](references/pipelines-and-policies.md).

## What counts as done

Say which of these you actually observed, and keep them apart:

| Claim | Evidence |
|---|---|
| A component version is published | `forest components show <org>/<name>` lists it |
| An image was built and pushed | the CI job and registry; Forest knows nothing about it until a release references it |
| An annotation exists | the slug printed by `annotate`/`create` |
| A release was created | `forest release show <slug>` finds an intent; "no release intent found" means the annotation released nothing |
| It deployed to a destination | the **deploy** stage and that destination's step are `SUCCEEDED` in `release show` |
| The service runs the new code | the running revision, image digest or build SHA the service itself reports |

Green CI is not a deployment, and a `SUCCEEDED` destination row can belong to a *plan* stage. Forest's release status is not proof that the new code is serving.
