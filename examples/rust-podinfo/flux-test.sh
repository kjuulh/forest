#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# rust-podinfo Flux v2 destination end-to-end test
#
# Tests the forest/flux@1 destination by:
#   1. Creating a bare git repo at .flux-test/bare.git
#   2. Running release prepare + annotate + release (forest.cue has both
#      kubernetes AND flux destinations — no file swapping needed)
#   3. Verifying the gitops repo has correct Flux v2 structure
#   4. Displaying the gitops repo tree and key file contents
#
# Prerequisites:
#   - mise installed
#   - forest server running (mise run develop)
#   - organisation "rawpotion" created:
#       mise run forest -- organisation create --name rawpotion
#
# Usage:
#   cd examples/rust-podinfo
#   ./flux-test.sh
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PASS=0
FAIL=0
SKIP=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP + 1)); }

section() { echo ""; echo "=== $1 ==="; }

FLUX_TEST_DIR="$SCRIPT_DIR/.flux-test"
BARE_REPO="$FLUX_TEST_DIR/bare.git"
VERIFY_DIR="$FLUX_TEST_DIR/verify"

# --------------------------------------------------------------------------
# Cleanup handler — only cleans build artifacts, preserves .flux-test for
# inspection (use --clean flag or delete manually)
# --------------------------------------------------------------------------

cleanup() {
  rm -rf .forest
}
trap cleanup EXIT

# --------------------------------------------------------------------------
section "0. Setup: bare git repo + flux destinations"
# --------------------------------------------------------------------------

# Clean previous test artifacts
rm -rf "$FLUX_TEST_DIR"
mkdir -p "$FLUX_TEST_DIR"

# Create bare repo
git init --bare "$BARE_REPO" > /dev/null 2>&1

# Bootstrap: clone, add initial commit, push to create 'main' branch
BOOTSTRAP_DIR="$FLUX_TEST_DIR/bootstrap"
git clone "file://$BARE_REPO" "$BOOTSTRAP_DIR" > /dev/null 2>&1
(
  cd "$BOOTSTRAP_DIR"
  echo "# GitOps Repo" > README.md
  git add -A
  git -c user.name=test -c user.email=test@test commit -m "initial" > /dev/null 2>&1
  git push origin HEAD:main > /dev/null 2>&1
)
rm -rf "$BOOTSTRAP_DIR"

if [ -d "$BARE_REPO/refs" ]; then
  pass "bare git repo created"
else
  fail "bare git repo creation"
  exit 1
fi

# Delete old flux destinations (ignore errors if they don't exist)
for dest in flux-dev flux-staging flux-prod; do
  mise run forest -- destination delete --name "$dest" > /dev/null 2>&1 || true
done

# Create flux destinations with required metadata
for env_dest in "dev:flux-dev:dev-cluster-01" "staging:flux-staging:staging-cluster-01" "prod:flux-prod:prod-cluster-01"; do
  IFS=: read -r env name cluster <<< "$env_dest"
  echo "  Creating destination: $name (env=$env, cluster=$cluster)"
  if mise run forest -- destination create \
    --organisation rawpotion \
    --name "$name" \
    --environment "$env" \
    --type "forest/flux@1" \
    --metadata "cluster_name=$cluster" \
    --metadata "namespace=rust-podinfo" \
    --metadata "git_url=file://$BARE_REPO" \
    --metadata "git_branch=main" > /dev/null 2>&1; then
    pass "destination create: $name"
  else
    fail "destination create: $name"
  fi
done

# --------------------------------------------------------------------------
section "1. Verify forest.cue has both destination types"
# --------------------------------------------------------------------------

if grep -q 'forest/kubernetes@1' forest.cue && grep -q 'forest/flux@1' forest.cue; then
  pass "forest.cue has both kubernetes and flux destination types"
else
  fail "forest.cue missing one of kubernetes/flux destination types"
fi

if grep -q 'flux-dev' forest.cue && grep -q 'flux-staging' forest.cue && grep -q 'flux-prod' forest.cue; then
  pass "forest.cue has flux destination patterns for all envs"
else
  fail "forest.cue missing flux destination patterns"
fi

# --------------------------------------------------------------------------
section "2. Release prepare"
# --------------------------------------------------------------------------

echo "  Running: forest release prepare"
if mise run forest -- release prepare > /dev/null 2>&1; then
  pass "release prepare"
else
  fail "release prepare"
fi

# Check generated files exist for each env
for env in dev staging prod; do
  if [ -d ".forest/deployment/$env" ]; then
    file_count=$(find ".forest/deployment/$env" -type f | wc -l)
    pass "release prepare: $env ($file_count files)"
  else
    fail "release prepare: $env (directory missing)"
  fi
done

# Check that both destination types are present
if find .forest/deployment -type d -name "flux@1" | grep -q .; then
  pass "release prepare: uses forest/flux@1 template dir"
else
  fail "release prepare: expected flux@1 directories"
fi

if find .forest/deployment -type d -name "kubernetes@1" | grep -q .; then
  pass "release prepare: uses forest/kubernetes@1 template dir"
else
  fail "release prepare: expected kubernetes@1 directories"
fi

# Spot-check rendered content for flux destinations
if grep -q "replicas: 3" .forest/deployment/prod/*/forest/flux@1/20-deployment.yaml 2>/dev/null; then
  pass "release prepare: flux prod replicas=3"
else
  fail "release prepare: flux prod replicas=3"
fi

if grep -q "replicas: 1" .forest/deployment/dev/*/forest/flux@1/20-deployment.yaml 2>/dev/null; then
  pass "release prepare: flux dev replicas=1"
else
  fail "release prepare: flux dev replicas=1"
fi

# --------------------------------------------------------------------------
section "3. Release annotate"
# --------------------------------------------------------------------------

echo "  Running: forest release annotate"
ANNOTATE_OUTPUT=$(mise run forest -- release annotate \
  --context-title "Flux test release v0.1.0" \
  --context-description "Automated flux destination test" \
  --organisation rawpotion \
  --project-name rust-podinfo \
  --commit-sha "$(git rev-parse HEAD 2>/dev/null || echo test123)" \
  --commit-branch "$(git branch --show-current 2>/dev/null || echo main)" \
  --commit-message "test: flux destination e2e" \
  --version 0.1.0 2>&1)

SLUG=$(echo "$ANNOTATE_OUTPUT" | grep "published artifact:" | sed 's/.*published artifact: //')

if [ -n "$SLUG" ]; then
  pass "release annotate (slug: $SLUG)"
else
  fail "release annotate (no slug returned)"
  echo "  Output: $ANNOTATE_OUTPUT" | tail -5
fi

# --------------------------------------------------------------------------
section "4. Release to each environment"
# --------------------------------------------------------------------------

if [ -n "$SLUG" ]; then
  for env in dev staging prod; do
    echo "  Releasing to $env..."
    if mise run forest -- release "$SLUG" --environment "$env" 2>&1 | grep -q "Release completed successfully"; then
      pass "release to $env"
    else
      fail "release to $env"
    fi
  done
else
  skip "release to dev (no slug)"
  skip "release to staging (no slug)"
  skip "release to prod (no slug)"
fi

# --------------------------------------------------------------------------
section "5. Verify gitops repo structure"
# --------------------------------------------------------------------------

# Clone the bare repo to inspect
rm -rf "$VERIFY_DIR"
git clone "file://$BARE_REPO" "$VERIFY_DIR" > /dev/null 2>&1

echo "  Checking Flux v2 directory structure..."

# Check each environment
for env_data in "dev:flux-dev:dev-cluster-01:1" "staging:flux-staging:staging-cluster-01:2" "prod:flux-prod:prod-cluster-01:3"; do
  IFS=: read -r env dest cluster replicas <<< "$env_data"

  RELEASE_DIR="$VERIFY_DIR/releases/$env/$dest/$cluster/rust-podinfo/rawpotion-rust-podinfo"
  CLUSTER_DIR="$VERIFY_DIR/clusters/$env/$dest/$cluster/rust-podinfo/rawpotion-rust-podinfo"

  # Check releases directory has manifests
  if [ -f "$RELEASE_DIR/10-namespace.yaml" ]; then
    pass "gitops: $env releases/10-namespace.yaml exists"
  else
    fail "gitops: $env releases/10-namespace.yaml missing"
  fi

  if [ -f "$RELEASE_DIR/20-deployment.yaml" ]; then
    pass "gitops: $env releases/20-deployment.yaml exists"
  else
    fail "gitops: $env releases/20-deployment.yaml missing"
  fi

  if [ -f "$RELEASE_DIR/30-service.yaml" ]; then
    pass "gitops: $env releases/30-service.yaml exists"
  else
    fail "gitops: $env releases/30-service.yaml missing"
  fi

  # Check replicas match environment
  if grep -q "replicas: $replicas" "$RELEASE_DIR/20-deployment.yaml" 2>/dev/null; then
    pass "gitops: $env deployment replicas=$replicas"
  else
    fail "gitops: $env deployment expected replicas=$replicas"
  fi

  # Check clusters directory has kustomization
  if [ -f "$CLUSTER_DIR/kustomization.yaml" ]; then
    pass "gitops: $env clusters/kustomization.yaml exists"
  else
    fail "gitops: $env clusters/kustomization.yaml missing"
  fi

  # Check kustomization CR content
  if grep -q "kind: Kustomization" "$CLUSTER_DIR/kustomization.yaml" 2>/dev/null; then
    pass "gitops: $env kustomization is Flux CR"
  else
    fail "gitops: $env kustomization not a Flux CR"
  fi

  if grep -q "targetNamespace: rust-podinfo" "$CLUSTER_DIR/kustomization.yaml" 2>/dev/null; then
    pass "gitops: $env kustomization targets rust-podinfo namespace"
  else
    fail "gitops: $env kustomization wrong targetNamespace"
  fi

  # Check the path in kustomization points to releases
  EXPECTED_PATH="./releases/$env/$dest/$cluster/rust-podinfo/rawpotion-rust-podinfo"
  if grep -q "path: $EXPECTED_PATH" "$CLUSTER_DIR/kustomization.yaml" 2>/dev/null; then
    pass "gitops: $env kustomization path correct"
  else
    fail "gitops: $env kustomization path wrong (expected $EXPECTED_PATH)"
  fi

  # Check .forest/ metadata directory
  FOREST_DIR="$RELEASE_DIR/.forest"
  if [ -d "$FOREST_DIR" ]; then
    pass "gitops: $env .forest/ metadata directory exists"
  else
    fail "gitops: $env .forest/ metadata directory missing"
  fi

  if [ -f "$FOREST_DIR/config.yaml" ]; then
    pass "gitops: $env .forest/config.yaml exists"
  else
    fail "gitops: $env .forest/config.yaml missing"
  fi

  if [ -f "$FOREST_DIR/release.yaml" ]; then
    pass "gitops: $env .forest/release.yaml exists"
  else
    fail "gitops: $env .forest/release.yaml missing"
  fi

  if [ -f "$FOREST_DIR/spec.yaml" ]; then
    pass "gitops: $env .forest/spec.yaml exists"
  else
    fail "gitops: $env .forest/spec.yaml missing"
  fi

  # Check release.yaml has slug
  if grep -q "slug" "$FOREST_DIR/release.yaml" 2>/dev/null; then
    pass "gitops: $env release.yaml has slug"
  else
    fail "gitops: $env release.yaml missing slug"
  fi

  # Check release.yaml has destination info
  if grep -q "destination:" "$FOREST_DIR/release.yaml" 2>/dev/null; then
    pass "gitops: $env release.yaml has destination info"
  else
    fail "gitops: $env release.yaml missing destination info"
  fi
done

# Check the README still exists (not clobbered)
if [ -f "$VERIFY_DIR/README.md" ]; then
  pass "gitops: README.md preserved"
else
  fail "gitops: README.md clobbered"
fi

# Check git history has release commits
COMMIT_COUNT=$(cd "$VERIFY_DIR" && git log --oneline | wc -l)
if [ "$COMMIT_COUNT" -ge 4 ]; then
  # 1 initial + 3 release commits (one per env)
  pass "gitops: $COMMIT_COUNT commits (initial + 3 releases)"
else
  fail "gitops: expected at least 4 commits, got $COMMIT_COUNT"
fi

# --------------------------------------------------------------------------
section "6. GitOps repo output"
# --------------------------------------------------------------------------

echo ""
echo "  Gitops repo location: $VERIFY_DIR"
echo "  Bare repo location:   $BARE_REPO"
echo ""

echo "  --- Directory tree ---"
(cd "$VERIFY_DIR" && find . -not -path './.git/*' -not -path './.git' | sort | sed 's|^./||' | while read -r path; do
  if [ -d "$VERIFY_DIR/$path" ]; then
    echo "    $path/"
  else
    echo "    $path"
  fi
done)

echo ""
echo "  --- Git log ---"
(cd "$VERIFY_DIR" && git log --oneline) | sed 's/^/    /'

echo ""
echo "  --- Sample: dev kustomization.yaml ---"
DEV_KUSTOMIZATION="$VERIFY_DIR/clusters/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo/kustomization.yaml"
if [ -f "$DEV_KUSTOMIZATION" ]; then
  cat "$DEV_KUSTOMIZATION" | sed 's/^/    /'
else
  echo "    (not found)"
fi

echo ""
echo "  --- Sample: dev 20-deployment.yaml ---"
DEV_DEPLOYMENT="$VERIFY_DIR/releases/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo/20-deployment.yaml"
if [ -f "$DEV_DEPLOYMENT" ]; then
  cat "$DEV_DEPLOYMENT" | sed 's/^/    /'
else
  echo "    (not found)"
fi

echo ""
echo "  --- Sample: dev .forest/release.yaml ---"
DEV_RELEASE_YAML="$VERIFY_DIR/releases/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo/.forest/release.yaml"
if [ -f "$DEV_RELEASE_YAML" ]; then
  cat "$DEV_RELEASE_YAML" | sed 's/^/    /'
else
  echo "    (not found)"
fi

echo ""
echo "  --- Sample: dev .forest/config.yaml ---"
DEV_CONFIG_YAML="$VERIFY_DIR/releases/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo/.forest/config.yaml"
if [ -f "$DEV_CONFIG_YAML" ]; then
  cat "$DEV_CONFIG_YAML" | sed 's/^/    /'
else
  echo "    (not found)"
fi

echo ""
echo "  Note: .flux-test/ is preserved for inspection."
echo "  To clean up: rm -rf examples/rust-podinfo/.flux-test"

# --------------------------------------------------------------------------
section "Results"
# --------------------------------------------------------------------------

TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo "  Total: $TOTAL  Pass: $PASS  Fail: $FAIL  Skip: $SKIP"
echo ""

if [ "$FAIL" -gt 0 ]; then
  echo "  SOME TESTS FAILED"
  exit 1
else
  echo "  ALL TESTS PASSED"
  exit 0
fi
