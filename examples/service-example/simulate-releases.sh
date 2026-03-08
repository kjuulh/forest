#!/usr/bin/env zsh
#
# Simulates realistic production release traffic against the service-example project.
# Assumes bootstrap.sh has already been run.
#
# Each run annotates a fresh artifact with random realistic metadata.
# The trigger (main + github_actions → pipeline) handles auto-releasing
# matching annotations. Non-matching ones sit as annotated artifacts.
#
# Usage:
#   ./simulate-releases.sh          # annotate one random release
#   ./simulate-releases.sh 10       # annotate 10 random releases
#   ./simulate-releases.sh loop     # annotate continuously with random delays
#
set -e

forest() { mise run forest "$@"; }

ORG=rawpotion
PROJECT=service-example

# ── Data pools ─────────────────────────────────────────────────────────

AUTHORS=(
  "kjuulh:contact@kasperhermansen.com"
  "mhoumann:martin@rawpotion.io"
  "aborg:anna@rawpotion.io"
  "tnielsen:thomas@rawpotion.io"
  "bot:ci-bot@rawpotion.io"
)

FEAT_COMMITS=(
  "feat: add WebSocket support for real-time notifications"
  "feat: implement rate limiting with sliding window"
  "feat: add OpenTelemetry tracing instrumentation"
  "feat: support multi-tenant API key scoping"
  "feat: add gRPC health check endpoints"
  "feat: implement graceful shutdown with drain period"
  "feat: add structured audit logging for compliance"
  "feat: support S3-compatible artifact storage backend"
  "feat: add webhook retry with exponential backoff"
  "feat: implement connection pooling with PgBouncer support"
)

FIX_COMMITS=(
  "fix: resolve connection leak under high concurrency"
  "fix: handle null pointer in auth token refresh"
  "fix: correct timezone handling in cron scheduler"
  "fix: prevent race condition in session store"
  "fix: resolve OOM on large payload deserialization"
  "fix: correct CORS header for preflight requests"
  "fix: handle database reconnect after failover"
  "fix: prevent duplicate event delivery on network retry"
  "fix: resolve deadlock in cache invalidation path"
  "fix: correct metric label cardinality for HTTP status codes"
)

REFACTOR_COMMITS=(
  "refactor: extract middleware chain into composable pipeline"
  "refactor: migrate from sync to async database driver"
  "refactor: consolidate error types into unified hierarchy"
  "refactor: replace hand-rolled JSON parser with serde"
  "refactor: split monolithic handler into domain services"
)

PERF_COMMITS=(
  "perf: add response caching for hot read paths"
  "perf: switch to zero-copy deserialization for large payloads"
  "perf: batch outbound HTTP calls with connection reuse"
  "perf: precompute aggregation views on write path"
  "perf: reduce allocations in inner request loop"
)

CHORE_COMMITS=(
  "chore: bump dependencies to latest security patches"
  "chore: update base image to alpine 3.19"
  "chore: rotate internal service credentials"
  "chore: migrate CI to new runner pool"
  "chore: clean up deprecated feature flags"
)

BRANCHES=(
  "main"
  "main"
  "main"
  "main"
  "feature/user-profiles"
  "feature/billing-v2"
  "feature/dark-mode"
  "feature/api-v3"
  "experiment/ml-ranking"
  "hotfix/urgent-fix"
  "release/v2.0"
)

SOURCE_TYPES=(github_actions github_actions github_actions gitlab_ci manual)

# ── Helpers ────────────────────────────────────────────────────────────

# Use /dev/urandom for randomness to avoid $RANDOM subshell seed issues.
rand_int() {
  # Returns a random integer in [0, $1)
  local max=$1
  local val
  val=$(od -An -tu4 -N4 /dev/urandom | tr -d ' ')
  echo $(( val % max ))
}

rand_element() {
  local -a arr=("${@}")
  local idx=$(( $(rand_int ${#arr[@]}) + 1 ))  # zsh 1-based
  echo "${arr[$idx]}"
}

rand_range() {
  local span=$(( $2 - $1 + 1 ))
  echo $(( $1 + $(rand_int $span) ))
}

rand_sha() {
  head -c 20 /dev/urandom | xxd -p
}

rand_version() {
  echo "$(rand_range 0 3).$(rand_range 0 20).$(rand_range 0 50)"
}

pick_commit() {
  local category=$(rand_range 1 10)
  if   (( category <= 4 )); then rand_element "${FEAT_COMMITS[@]}"
  elif (( category <= 7 )); then rand_element "${FIX_COMMITS[@]}"
  elif (( category <= 8 )); then rand_element "${PERF_COMMITS[@]}"
  elif (( category <= 9 )); then rand_element "${REFACTOR_COMMITS[@]}"
  else                           rand_element "${CHORE_COMMITS[@]}"
  fi
}

# ── Annotate a random release ─────────────────────────────────────────

annotate_random() {
  local msg=$(pick_commit)
  local branch=$(rand_element "${BRANCHES[@]}")
  local source=$(rand_element "${SOURCE_TYPES[@]}")
  local author_entry=$(rand_element "${AUTHORS[@]}")
  local author_name="${author_entry%%:*}"
  local version=$(rand_version)
  local sha=$(rand_sha)
  local pr_num=$(rand_range 60 999)
  local run_id=$(rand_range 8800000000 9999999999)

  # Suffix for non-main branches
  local version_suffix=""
  if [[ "$branch" != "main" ]]; then
    version_suffix="-${branch##*/}"
  fi

  local title="${msg} (#${pr_num})"
  local desc="Branch: ${branch}. Source: ${source}. Author: ${author_name}."

  echo "==> [${branch}] ${msg}  (${source}, ${author_name})"

  forest release annotate \
    --organisation "$ORG" \
    --project-name "$PROJECT" \
    --context-title "$title" \
    --context-description "$desc" \
    --context-web "https://github.com/rawpotion/service-example/pull/${pr_num}" \
    --context-pr "https://github.com/rawpotion/service-example/pull/${pr_num}" \
    --commit-sha "$sha" \
    --commit-branch "$branch" \
    --commit-message "${msg} (#${pr_num})" \
    --version "${version}${version_suffix}" \
    --repo-url "https://github.com/rawpotion/service-example" \
    --source-type "$source" \
    --run-url "https://github.com/rawpotion/service-example/actions/runs/${run_id}" \
    --metadata "image=ghcr.io/rawpotion/service-example:${version}${version_suffix}" \
    --metadata "chart_version=1.$(rand_range 0 5).$(rand_range 0 10)"
}

# ── Main ──────────────────────────────────────────────────────────────

case "${1:-1}" in
  loop)
    echo "Running in continuous mode (Ctrl-C to stop)"
    while true; do
      echo ""
      echo "─── $(date '+%H:%M:%S') ───────────────────────────────────"
      annotate_random
      delay=$(rand_range 2 10)
      echo "    (next in ${delay}s)"
      sleep "$delay"
    done
    ;;
  *)
    count="${1:-1}"
    for i in $(seq 1 "$count"); do
      if (( count > 1 )); then
        echo ""
        echo "─── ${i}/${count} ────────────────────────────────────────"
      fi
      annotate_random
    done
    ;;
esac

echo ""
echo "Done. Use 'forest project releases -o rawpotion -p service-example --all' to see status."
