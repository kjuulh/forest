# CI/CD Integration

Forest is designed to be driven from CI/CD pipelines. This guide covers common integration patterns.

## Authentication

Use an app token for CI/CD:

```bash
# Generate a token (one-time setup)
forest organisation app create --name "ci-bot"
# Store the token as a CI secret
```

Set the token in your pipeline:

```bash
export FOREST_TOKEN="<your-app-token>"
export FOREST_SERVER="https://forest.example.com:4040"
```

## Basic Pipeline

### GitHub Actions

```yaml
name: Deploy
on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Forest
        run: cargo install forest-cli

      - name: Annotate Release
        env:
          FOREST_TOKEN: ${{ secrets.FOREST_TOKEN }}
          FOREST_SERVER: ${{ secrets.FOREST_SERVER }}
        run: |
          forest release annotate \
            --organisation my-org \
            --project-name my-service \
            --context-title "${{ github.event.head_commit.message }}" \
            --commit-sha "${{ github.sha }}" \
            --commit-branch "${{ github.ref_name }}" \
            --source-type ci \
            --run-url "${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}"
```

## Trigger-Based Flow

The recommended pattern is to use [triggers](../concepts/triggers.md) instead of explicit release commands in CI. Your CI pipeline only annotates — triggers handle the rest:

```yaml
# CI only annotates
- name: Annotate
  run: |
    forest release annotate \
      --organisation my-org \
      --project-name my-service \
      --context-title "$(git log -1 --format=%s)" \
      --commit-sha "$(git rev-parse HEAD)" \
      --commit-branch "$(git branch --show-current)" \
      --source-type ci
```

Configure triggers on the server side:

```bash
# Auto-deploy to staging on main
forest project trigger create ci-staging \
  --branch "^main$" \
  --source-type "^ci$" \
  --target-environment staging

# Auto-deploy to prod via pipeline on tags
forest project trigger create ci-prod \
  --branch "^v[0-9]" \
  --target-environment prod \
  --use-pipeline
```

This separates **what** gets deployed (CI annotation) from **where** and **how** (server-side triggers and policies).

## Explicit Release Flow

For full control, annotate and release explicitly:

```yaml
- name: Release to staging
  run: |
    forest release release \
      --organisation my-org \
      --project my-service \
      --environment staging

- name: Wait for staging
  run: |
    forest release wait "$INTENT_ID"
```

## Policy Evaluation

Before releasing, check if policies allow it:

```yaml
- name: Check policies
  run: |
    forest project policy evaluate \
      --organisation my-org \
      --project my-service \
      --environment prod
```

## Release Context

The annotation captures rich metadata about the CI context:

| Flag | Description | Example |
|------|-------------|---------|
| `--source-type` | Where the release came from | `ci`, `manual`, `webhook` |
| `--source-username` | Who triggered it | `ci-bot` |
| `--source-email` | Email of the triggerer | `ci@example.com` |
| `--run-url` | Link back to the CI run | GitHub Actions URL |
| `--context-title` | Human-readable title | Commit message |
| `--context-description` | Longer description | PR body |
| `--context-web` | Link to the change | Commit URL |
| `--context-pr` | Pull request link | PR URL |
| `--commit-sha` | Exact commit | `abc123def` |
| `--commit-branch` | Source branch | `main` |
| `--commit-message` | Full commit message | `Add feature X` |

## Event Streaming

Subscribe to release events for notifications or dashboards:

```bash
forest notifications subscribe \
  --organisation my-org \
  --project my-service \
  --resource-types release \
  --actions status_changed
```
