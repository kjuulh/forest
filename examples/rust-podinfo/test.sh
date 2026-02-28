#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# rust-podinfo end-to-end test script
#
# This script exercises the full developer workflow using only `forest`
# commands and `curl` for HTTP verification.
#
# Prerequisites:
#   - mise installed
#   - forest server running (mise run develop)
#   - organisation "rawpotion" created:
#       mise run forest -- organisation create --name rawpotion
#   - destinations created:
#       mise run forest -- destination create --organisation rawpotion --name k8s-dev     --environment dev     --type "forest/kubernetes@1"
#       mise run forest -- destination create --organisation rawpotion --name k8s-staging --environment staging --type "forest/kubernetes@1"
#       mise run forest -- destination create --organisation rawpotion --name k8s-prod    --environment prod    --type "forest/kubernetes@1"
#
# Usage:
#   cd examples/rust-podinfo
#   ./test.sh              # run everything
#   ./test.sh --no-server  # skip steps that require the forest gRPC server
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

NO_SERVER=false
for arg in "$@"; do
  case "$arg" in
    --no-server) NO_SERVER=true ;;
  esac
done

PASS=0
FAIL=0
SKIP=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP + 1)); }

section() { echo ""; echo "=== $1 ==="; }

# --------------------------------------------------------------------------
section "1. Forest run commands"
# --------------------------------------------------------------------------

for cmd in build validate test status compile docker-build; do
  echo "  Running: forest run $cmd"
  if mise run forest -- run "$cmd" > /dev/null 2>&1; then
    pass "forest run $cmd"
  else
    fail "forest run $cmd"
  fi
done

# --------------------------------------------------------------------------
section "2. Dev server (forest run dev + curl)"
# --------------------------------------------------------------------------

echo "  Starting: forest run dev"
mise run forest -- run dev &>/tmp/test-podinfo.log &
DEV_PID=$!

cleanup_dev() {
  kill "$DEV_PID" 2>/dev/null || true
  wait "$DEV_PID" 2>/dev/null || true
}
trap cleanup_dev EXIT

# Wait for server to be ready
for i in $(seq 1 30); do
  if curl -sf http://localhost:8080/ > /dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

test_endpoint() {
  local name="$1" url="$2" expected="$3"
  local body
  body=$(curl -sf "$url" 2>/dev/null || echo "CURL_FAILED")
  if [ "$body" = "CURL_FAILED" ]; then
    fail "$name (connection failed)"
    return
  fi
  if echo "$body" | grep -q "$expected"; then
    pass "$name"
  else
    fail "$name (expected '$expected', got: $body)"
  fi
}

test_endpoint "GET /"        "http://localhost:8080/"        '"name":"rust-podinfo"'
test_endpoint "GET /version" "http://localhost:8080/version" '"version"'
test_endpoint "GET /env"     "http://localhost:8080/env"     '"env"'
test_endpoint "GET /healthz" "http://localhost:8081/healthz" '"status":"ok"'
test_endpoint "GET /readyz"  "http://localhost:8081/readyz"  '"status":"ready"'

cleanup_dev
trap - EXIT

# --------------------------------------------------------------------------
section "3. Release prepare"
# --------------------------------------------------------------------------

echo "  Running: forest release prepare"
if mise run forest -- release prepare > /dev/null 2>&1; then
  pass "release prepare"
else
  fail "release prepare"
fi

# Check generated files
for env in dev staging prod; do
  if [ -d ".forest/deployment/$env" ]; then
    file_count=$(find ".forest/deployment/$env" -type f | wc -l)
    pass "release prepare: $env ($file_count files)"
  else
    fail "release prepare: $env (directory missing)"
  fi
done

# Spot-check rendered content
if grep -q "replicas: 3" .forest/deployment/prod/*/forest/kubernetes@1/20-deployment.yaml 2>/dev/null; then
  pass "release prepare: prod replicas=3"
else
  fail "release prepare: prod replicas=3"
fi

if grep -q "replicas: 1" .forest/deployment/dev/*/forest/kubernetes@1/20-deployment.yaml 2>/dev/null; then
  pass "release prepare: dev replicas=1"
else
  fail "release prepare: dev replicas=1"
fi

# --------------------------------------------------------------------------
section "4. Release annotate + release (requires server)"
# --------------------------------------------------------------------------

if [ "$NO_SERVER" = true ]; then
  skip "release annotate (--no-server)"
  skip "release to dev (--no-server)"
  skip "release to staging (--no-server)"
  skip "release to prod (--no-server)"
else
  echo "  Running: forest release annotate"
  ANNOTATE_OUTPUT=$(mise run forest -- release annotate \
    --context-title "Test release v0.1.0" \
    --context-description "Automated test" \
    --organisation rawpotion \
    --project-name rust-podinfo \
    --commit-sha "$(git rev-parse HEAD 2>/dev/null || echo test123)" \
    --commit-branch "$(git branch --show-current 2>/dev/null || echo main)" \
    --commit-message "test: automated release test" \
    --version 0.1.0 2>&1)

  SLUG=$(echo "$ANNOTATE_OUTPUT" | grep "published artifact:" | sed 's/.*published artifact: //')

  if [ -n "$SLUG" ]; then
    pass "release annotate (slug: $SLUG)"
  else
    fail "release annotate (no slug returned)"
    # Print output for debugging
    echo "$ANNOTATE_OUTPUT" | tail -5
  fi

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
fi

# --------------------------------------------------------------------------
# Cleanup
# --------------------------------------------------------------------------

rm -rf .forest

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
