#!/usr/bin/env zsh
#
set -e

forest() { mise run forest "$@"; }

# ── Identity & organisation ─────────────────────────────────────────

echo "Creating user"
FOREST_PASSWORD=Something852456 forest auth register \
  --username hermansen \
  --email contact@kjuulh.io

echo "Creating organisation"
forest organisation create --name rawpotion

# ── Environments ────────────────────────────────────────────────────

echo "Creating environments"
forest environment create --name dev --description "Development — feature branches and integration testing"
forest environment create --name staging --description "Staging — pre-production verification"
forest environment create --name prod --description "Production"

# ── Destinations ────────────────────────────────────────────────────

echo "Creating destinations"
for name env in \
  infrastructure-dev/1    dev \
  infrastructure-dev/2    dev \
  infrastructure-staging/1 staging \
  infrastructure-prod/1   prod \
  infrastructure-prod/2   prod
do
  forest destination create \
    --name "$name" \
    --type forest/terraform@1 \
    --environment "$env" \
    --organisation rawpotion \
    --metadata "environment=$env"
done

# ── Project ─────────────────────────────────────────────────────────

echo "Creating project"
forest project create --organisation rawpotion --project service-example

# ── Release pipeline (dev → staging → prod) ─────────────────────────

echo "Creating release pipeline"
forest project pipeline create \
  --organisation rawpotion \
  --project service-example \
  --name "standard-rollout" \
  --stages-json '{
    "deploy-dev": {
      "type": "deploy",
      "environment": "dev",
      "depends_on": []
    },
    "soak-dev": {
      "type": "wait",
      "duration_seconds": 3,
      "depends_on": ["deploy-dev"]
    },
    "deploy-staging": {
      "type": "deploy",
      "environment": "staging",
      "depends_on": ["soak-dev"]
    },
    "soak-staging": {
      "type": "wait",
      "duration_seconds": 5,
      "depends_on": ["deploy-staging"]
    },
    "deploy-prod": {
      "type": "deploy",
      "environment": "prod",
      "depends_on": ["soak-staging"]
    }
  }'

# ── Trigger (main branch → pipeline) ─────────────────────────────────

echo "Creating trigger"
forest project trigger create \
  --organisation rawpotion \
  --project service-example \
  --name "deploy-main-via-pipeline" \
  --branch '^main$' \
  --env dev --env staging --env prod \
  --use-pipeline

# ── Deployment policies (guardrails) ──────────────────────────────────

echo "Creating policies"
forest project policy create \
  --organisation rawpotion \
  --project service-example \
  --name "soak-dev-to-staging" \
  --type soak_time \
  --source-environment dev \
  --target-environment staging \
  --duration 5

forest project policy create \
  --organisation rawpotion \
  --project service-example \
  --name "soak-staging-to-prod" \
  --type soak_time \
  --source-environment staging \
  --target-environment prod \
  --duration 10

forest project policy create \
  --organisation rawpotion \
  --project service-example \
  --name "prod-main-only" \
  --type branch_restriction \
  --target-environment prod \
  --branch-pattern '^main$'

# ── Prepare deployment artifacts ─────────────────────────────────────

echo "Preparing deployment artifacts"
forest release prepare

# ── Annotate releases ────────────────────────────────────────────────

echo "Annotating release 1 — auth middleware"
forest release annotate \
  --organisation rawpotion \
  --project-name service-example \
  --context-title "feat: add user authentication middleware" \
  --context-description "Implements JWT-based auth with refresh token rotation and RBAC middleware for API routes." \
  --context-web "https://github.com/rawpotion/service-example/pull/42" \
  --context-pr "https://github.com/rawpotion/service-example/pull/42" \
  --commit-sha "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0" \
  --commit-branch "main" \
  --commit-message "feat: add user authentication middleware (#42)" \
  --version "0.3.0" \
  --repo-url "https://github.com/rawpotion/service-example" \
  --source-type "github_actions" \
  --run-url "https://github.com/rawpotion/service-example/actions/runs/8834210095" \
  --metadata "image=ghcr.io/rawpotion/service-example:0.3.0" \
  --metadata "chart_version=1.2.0"

echo "Annotating release 2 — config refactor"
forest release annotate \
  --organisation rawpotion \
  --project-name service-example \
  --context-title "refactor: extract config into environment-aware module" \
  --context-description "Moves hardcoded connection strings and feature flags into a structured config module that reads from env vars with sensible defaults." \
  --context-web "https://github.com/rawpotion/service-example/pull/47" \
  --context-pr "https://github.com/rawpotion/service-example/pull/47" \
  --commit-sha "b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1" \
  --commit-branch "main" \
  --commit-message "refactor: extract config into environment-aware module (#47)" \
  --version "0.3.1" \
  --repo-url "https://github.com/rawpotion/service-example" \
  --source-type "github_actions" \
  --run-url "https://github.com/rawpotion/service-example/actions/runs/8841557321" \
  --metadata "image=ghcr.io/rawpotion/service-example:0.3.1" \
  --metadata "chart_version=1.2.0"

echo "Annotating release 3 — bugfix"
forest release annotate \
  --organisation rawpotion \
  --project-name service-example \
  --context-title "fix: prevent duplicate webhook delivery on retry" \
  --context-description "Adds idempotency key tracking to the webhook dispatcher so retried deliveries are de-duplicated at the receiver." \
  --context-web "https://github.com/rawpotion/service-example/pull/51" \
  --context-pr "https://github.com/rawpotion/service-example/pull/51" \
  --commit-sha "c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2" \
  --commit-branch "main" \
  --commit-message "fix: prevent duplicate webhook delivery on retry (#51)" \
  --version "0.3.2" \
  --repo-url "https://github.com/rawpotion/service-example" \
  --source-type "github_actions" \
  --run-url "https://github.com/rawpotion/service-example/actions/runs/8852930174" \
  --metadata "image=ghcr.io/rawpotion/service-example:0.3.2" \
  --metadata "chart_version=1.2.1"

echo "Annotating release 4 — performance improvement"
forest release annotate \
  --organisation rawpotion \
  --project-name service-example \
  --context-title "perf: batch database writes for event ingestion" \
  --context-description "Replaces per-event INSERT with batched writes using COPY protocol, reducing p99 latency from 120ms to 8ms under load." \
  --context-web "https://github.com/rawpotion/service-example/pull/58" \
  --context-pr "https://github.com/rawpotion/service-example/pull/58" \
  --commit-sha "d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3" \
  --commit-branch "main" \
  --commit-message "perf: batch database writes for event ingestion (#58)" \
  --version "0.4.0" \
  --repo-url "https://github.com/rawpotion/service-example" \
  --source-type "github_actions" \
  --run-url "https://github.com/rawpotion/service-example/actions/runs/8867441290" \
  --metadata "image=ghcr.io/rawpotion/service-example:0.4.0" \
  --metadata "chart_version=1.3.0"

echo ""
echo "Bootstrap complete."
echo "  Organisation: rawpotion"
echo "  Project:      service-example"
echo "  Environments: dev, staging, prod"
echo "  Destinations: 5 (2 dev, 1 staging, 2 prod)"
echo "  Pipeline:     standard-rollout (dev → soak → staging → soak → prod)"
echo "  Trigger:      deploy-main-via-pipeline (main branch, GitHub Actions)"
echo "  Policies:     soak-dev-to-staging (30s), soak-staging-to-prod (60s), prod-main-only"
echo "  Artifacts:    4 annotated releases (v0.3.0 – v0.4.0)"
