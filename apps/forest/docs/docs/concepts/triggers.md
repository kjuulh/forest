# Triggers

Triggers turn release annotations into releases. When an annotation matches a trigger's patterns, Forest releases it, either straight to the trigger's targets or through the project's pipeline.

## How triggers work

1. CI (or a person) annotates a release: `forest release annotate`, or `forest release create` without a target.
2. Forest evaluates every enabled trigger of the project against the annotation.
3. Each matching trigger releases the annotated artifact.

`forest release annotate --annotation-only` skips trigger evaluation. Since v0.3.17, `forest release create` with `--environment` or `--destination` also skips triggers **unless `--skip-commit` is supplied**. With `--skip-commit`, even `-e dev` enables triggers and can deploy prod. Released v0.3.16 requires an environment for `create`, skips triggers, and `--skip-commit` releases nothing; use `prepare` + `annotate` for its trigger flow.

## Pattern matching

Every pattern is a **regular expression, tested unanchored**: `--branch main` matches `feature/maintenance`. Anchor it as `^main$`. All patterns given must match. A pattern whose field is missing from the annotation does not match, and an invalid regex never matches.

| Flag | Matches against |
|---|---|
| `--branch` | commit branch |
| `--title` | annotation title |
| `--author` | source username or email |
| `--commit-message` | commit message |
| `--source-type` | source type (`ci`, `local`, `github_actions`, …) |

## Targets

| Flag | Effect |
|---|---|
| `-e`, `--env` | environment(s) to release to (repeatable) |
| `-d`, `--dest` | destination(s) to release to (repeatable) |
| `--use-pipeline` | start the project's pipeline instead |
| `--force` | cancel queued releases for the same destinations; it does **not** bypass policies |

**With `--use-pipeline` the trigger's own `--env`/`--dest` are ignored, and the whole pipeline runs**, including any prod stages. The trigger list still shows `environments: dev` for it. Read the pipeline, not the trigger's name, to know where a trigger can deploy.

## Examples

Deploy every CI build of `main` through the pipeline:

```bash
forest project trigger create -o my-org -p my-service \
  --name deploy-main-via-pipeline \
  --branch '^main$' --source-type '^ci$' --use-pipeline
```

Deploy release tags straight to staging:

```bash
forest project trigger create -o my-org -p my-service \
  --name tags-to-staging --branch '^v[0-9]+\.' --env staging
```

## Interaction with policies

Triggers create releases; [policies](policies.md) still apply to them. With the server enforcement changes in v0.3.17, a trigger-created release to an environment with a branch restriction is refused for a disallowed branch, and in a pipeline the stage for that environment fails. Released v0.3.16 does not enforce pipeline branch restrictions; updating only the CLI does not close that gap. `--force` never bypasses a policy.

## CLI commands

```bash
forest project trigger create -o <org> -p <project> --name <name> [patterns] [targets]
forest project trigger list   -o <org> -p <project>
forest project trigger update -o <org> -p <project> --name <name> [options]
forest project trigger delete -o <org> -p <project> --name <name>
```
