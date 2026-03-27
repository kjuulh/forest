# Triggers

Triggers automate releases based on patterns in release annotations. When an annotation matches a trigger's patterns, Forest automatically creates a release to the trigger's target environments and destinations.

## How Triggers Work

1. A release annotation is created (e.g., from CI/CD)
2. Forest evaluates all triggers for the project
3. Triggers whose patterns match the annotation fire automatically
4. Each fired trigger creates releases to its configured targets

## Pattern Matching

Triggers match on annotation fields using **regex patterns**. All specified patterns must match (AND semantics):

| Pattern Field | Matches Against |
|---------------|----------------|
| `branch` | Commit branch (e.g., `^main$`, `^release/.*`) |
| `title` | Annotation context title |
| `author` | Source username or email |
| `commit_message` | Commit message text |
| `source_type` | Source type (e.g., `ci`, `manual`, `webhook`) |

If a pattern field is not specified, it matches everything.

## Examples

### Deploy to staging on every commit to main

```bash
forest project trigger create deploy-staging \
  --organisation my-org \
  --project my-service \
  --branch "^main$" \
  --target-environment staging
```

### Deploy to prod only from release tags

```bash
forest project trigger create deploy-prod \
  --organisation my-org \
  --project my-service \
  --branch "^v[0-9]+\\." \
  --target-environment prod
```

### Deploy specific destinations on hotfix branches

```bash
forest project trigger create hotfix \
  --organisation my-org \
  --project my-service \
  --branch "^hotfix/" \
  --target-destination k8s-prod-eu \
  --force-release
```

## Trigger Options

| Option | Description |
|--------|-------------|
| `--target-environment` | Environment(s) to deploy to |
| `--target-destination` | Specific destination(s) to deploy to |
| `--force-release` | Cancel any queued releases and deploy immediately |
| `--use-pipeline` | Route the release through the project's pipeline |

## Trigger Evaluation

Triggers are evaluated during `forest release annotate` unless `--annotation-only` is set. The evaluation:

1. Loads all enabled triggers for the project
2. Tests each pattern against the annotation metadata
3. For matching triggers, creates release intents for the target destinations
4. If `use_pipeline` is set, creates a pipeline intent instead of direct releases

## CLI Commands

```bash
forest project trigger create <name> [options]
forest project trigger list --organisation my-org --project my-service
forest project trigger update <name> [options]
forest project trigger delete <name>
```

## Interaction with Policies

Triggers create releases, but [policies](policies.md) still apply. A trigger-created release will be blocked if it violates a policy — unless `--force-release` is set.
