#!/usr/bin/env bash
# forest-ci entrypoint — shared between the woodpecker plugin and a
# future GitHub Action. Inputs are read from env vars under three
# accepted prefixes (precedence high → low): explicit FOREST_* /
# PLUGIN_* (woodpecker) / INPUT_* (GitHub). Same binary, same
# contract, regardless of host CI.
#
# Actions:
#   release-create   — invoke `forest release create` per project
#   release-prepare  — invoke `forest release prepare` per project
#   release-annotate — invoke `forest release annotate` per project
#
# Layout modes (auto-detected via projects_dir):
#   - PROJECTS_DIR contains a forest.cue       → treat as a single project
#   - PROJECTS_DIR contains subdirs with forest.cue → iterate (monorepo)
#
# Required inputs:
#   action            — one of the actions above
#   forest_server     — forest server URL
#   forest_token      — token with release/annotate perms
#
# Required for release-create:
#   environment       — e.g. dev | prod
#
# Optional inputs:
#   projects_dir      — default "deployment/projects" (or "." for
#                       single-project repos)
#   image_tag         — shorthand for --set kjuulh/service.tag=<value>
#   extra_sets        — newline-separated key=value list (more --set)
#   cue_registry      — derived from forest_server when empty
#   rust_log          — default "forest=info,component=info"

set -euo pipefail

# ── input resolution ────────────────────────────────────────────────
# Read X from $FOREST_X, $PLUGIN_X, $INPUT_X in that order.
read_input() {
  local upper=$1
  local v
  v="$(printenv "FOREST_$upper" 2>/dev/null || true)"
  [ -n "$v" ] && { printf '%s' "$v"; return; }
  v="$(printenv "PLUGIN_$upper" 2>/dev/null || true)"
  [ -n "$v" ] && { printf '%s' "$v"; return; }
  v="$(printenv "INPUT_$upper" 2>/dev/null || true)"
  [ -n "$v" ] && { printf '%s' "$v"; return; }
  return 0
}

require() {
  local name=$1 val=$2
  if [ -z "$val" ]; then
    echo "forest-ci: missing required input '$name'" >&2
    exit 2
  fi
}

ACTION="$(read_input ACTION)"
FOREST_SERVER="$(read_input FOREST_SERVER)"
FOREST_TOKEN="$(read_input FOREST_TOKEN)"
ENVIRONMENT="$(read_input ENVIRONMENT)"
PROJECTS_DIR="$(read_input PROJECTS_DIR)"
IMAGE_TAG="$(read_input IMAGE_TAG)"
EXTRA_SETS="$(read_input EXTRA_SETS)"
CUE_REGISTRY="$(read_input CUE_REGISTRY)"
RUST_LOG_IN="$(read_input RUST_LOG)"

: "${PROJECTS_DIR:=deployment/projects}"
: "${RUST_LOG_IN:=forest=info,component=info}"

require action "$ACTION"
require forest_server "$FOREST_SERVER"
require forest_token "$FOREST_TOKEN"

# Derive CUE_REGISTRY when not supplied. Mirrors the forest CLI's own
# rule: forest.sh → registry.<server-host>, with cuelang as fallback.
if [ -z "$CUE_REGISTRY" ]; then
  host="${FOREST_SERVER#https://}"
  host="${host#http://}"
  host="${host%%/*}"
  CUE_REGISTRY="forest.sh=registry.${host},registry.cuelang.org"
fi

export FOREST_SERVER FOREST_TOKEN CUE_REGISTRY
export RUST_LOG="$RUST_LOG_IN"

# ── CI metadata auto-detection ──────────────────────────────────────
# Same commit info under either host. Explicit env overrides win so a
# pipeline can pin metadata when running outside a known CI.
detect_ci() {
  if [ -n "${CI_COMMIT_SHA:-}" ]; then
    : "${CI_SHA:=$CI_COMMIT_SHA}"
    : "${CI_BRANCH:=${CI_COMMIT_BRANCH:-}}"
    : "${CI_MSG:=${CI_COMMIT_MESSAGE:-}}"
    : "${CI_REPO:=${CI_REPO_URL:-}}"
    : "${CI_RUN:=${CI_PIPELINE_URL:-}}"
    : "${CI_SOURCE_TYPE:=woodpecker}"
  elif [ -n "${GITHUB_SHA:-}" ]; then
    : "${CI_SHA:=$GITHUB_SHA}"
    : "${CI_BRANCH:=${GITHUB_REF_NAME:-}}"
    : "${CI_MSG:=${GITHUB_EVENT_HEAD_COMMIT_MESSAGE:-}}"
    : "${CI_REPO:=${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}}"
    : "${CI_RUN:=${GITHUB_SERVER_URL:-}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-}}"
    : "${CI_SOURCE_TYPE:=github-actions}"
  else
    echo "forest-ci: no known CI environment — set CI_COMMIT_SHA or GITHUB_SHA" >&2
    exit 2
  fi
}
detect_ci

# ── project discovery ──────────────────────────────────────────────
# A "project" is any dir containing a forest.cue. PROJECTS_DIR may be
# the project itself (single-project repo) or a directory of projects.
if [ -f "$PROJECTS_DIR/forest.cue" ]; then
  projects=("$PROJECTS_DIR")
else
  projects=()
  for entry in "$PROJECTS_DIR"/*/; do
    [ -d "$entry" ] || continue
    if [ -f "$entry/forest.cue" ]; then
      projects+=("$entry")
    fi
  done
fi

if [ "${#projects[@]}" -eq 0 ]; then
  echo "forest-ci: no projects found under '$PROJECTS_DIR' (no forest.cue)" >&2
  exit 2
fi

# Compose the --set flags once; reused per project.
set_args=()
if [ -n "$IMAGE_TAG" ]; then
  set_args+=(--set "kjuulh/service.tag=$IMAGE_TAG")
fi
if [ -n "$EXTRA_SETS" ]; then
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    set_args+=(--set "$line")
  done <<<"$EXTRA_SETS"
fi

# ── dispatch ──────────────────────────────────────────────────────
echo "forest-ci: action=$ACTION projects=${#projects[@]} source=$CI_SOURCE_TYPE sha=$CI_SHA"

run_release_create() {
  require environment "$ENVIRONMENT"
  for proj in "${projects[@]}"; do
    echo "==> release create: $proj"
    ( cd "$proj" && forest release create \
        --environment "$ENVIRONMENT" \
        "${set_args[@]}" \
        --commit-sha "$CI_SHA" \
        --commit-branch "$CI_BRANCH" \
        --commit-message "$CI_MSG" \
        --version "$CI_SHA" \
        --repo-url "$CI_REPO" \
        --source-type "$CI_SOURCE_TYPE" \
        --run-url "$CI_RUN" )
  done
}

run_release_prepare() {
  for proj in "${projects[@]}"; do
    echo "==> release prepare: $proj"
    ( cd "$proj" && forest release prepare "${set_args[@]}" )
  done
}

run_release_annotate() {
  for proj in "${projects[@]}"; do
    org=$(cd "$proj" && cue eval -e project.organisation --out json . | tr -d '"')
    name=$(cd "$proj" && cue eval -e project.name --out json . | tr -d '"')
    echo "==> release annotate: $org/$name ($proj)"
    ( cd "$proj" && forest release annotate \
        --organisation "$org" \
        --project-name "$name" \
        --context-title "$CI_MSG" \
        --context-description "Branch: $CI_BRANCH. Commit: $CI_SHA" \
        --context-web "$CI_RUN" \
        --commit-sha "$CI_SHA" \
        --commit-branch "$CI_BRANCH" \
        --commit-message "$CI_MSG" \
        --version "$CI_SHA" \
        --repo-url "$CI_REPO" \
        --source-type "$CI_SOURCE_TYPE" \
        --run-url "$CI_RUN" )
  done
}

case "$ACTION" in
  release-create)   run_release_create   ;;
  release-prepare)  run_release_prepare  ;;
  release-annotate) run_release_annotate ;;
  *) echo "forest-ci: unknown action '$ACTION'" >&2; exit 2 ;;
esac
