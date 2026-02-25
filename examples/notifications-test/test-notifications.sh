#!/usr/bin/env bash
#
# End-to-end test for the Forest notification system.
#
# Prerequisites:
#   1. mise run local:up       # start postgres
#   2. mise run db:migrate     # run migrations
#   3. mise run develop        # start forest-server (in a separate terminal)
#
# Usage (from repo root):
#   bash examples/notifications-test/test-notifications.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

FOREST="mise run forest --"
export FOREST_SERVER="${FOREST_SERVER:-http://localhost:4040}"
export FOREST_PASSWORD="${FOREST_PASSWORD:-Testpass123!}"

RAND_SUFFIX="$(date +%s)"
USERNAME="testuser-${RAND_SUFFIX}"
EMAIL="test-${RAND_SUFFIX}@example.com"
ORG="testorg${RAND_SUFFIX}"
PROJECT="notif-test"

echo "=== Forest Notification System Test ==="
echo ""
echo "Server:  ${FOREST_SERVER}"
echo "User:    ${USERNAME}"
echo "Org:     ${ORG}"
echo "Project: ${PROJECT}"
echo ""

# ── Step 1: Register user ────────────────────────────────────────
echo "--- Step 1: Register user ---"
$FOREST auth register --username "${USERNAME}" --email "${EMAIL}" 2>&1 | grep -E "Registered|Error"
echo ""

# ── Step 2: Create organisation ──────────────────────────────────
echo "--- Step 2: Create organisation ---"
$FOREST organisation create --name "${ORG}" 2>&1 | grep -E "ID:|Name:|Error"
echo ""

# ── Step 3: Create kubernetes destination (no-op) ────────────────
DEST_NAME="k8s-${RAND_SUFFIX}"
echo "--- Step 3: Create destination (${DEST_NAME}) ---"
$FOREST destination create \
  --organisation "${ORG}" \
  --name "${DEST_NAME}" \
  --environment "dev" \
  --type "forest/kubernetes@1" 2>&1 | grep -v "^warning\|^  -->\|^   =\|^   |\|Compiling\|Finished\|Running" || true
echo "Destination created."
echo ""

# ── Step 4: Start notification listener in background ────────────
echo "--- Step 4: Start notification listener (background) ---"
NOTIFICATION_LOG="/tmp/forest-notifications-${RAND_SUFFIX}.log"
$FOREST notifications listen > "${NOTIFICATION_LOG}" 2>&1 &
LISTEN_PID=$!
echo "Listener PID: ${LISTEN_PID}"
sleep 4
echo ""

# ── Step 5: Annotate artifact ────────────────────────────────────
echo "--- Step 5: Annotate artifact ---"
cd "${SCRIPT_DIR}"
ANNOTATE_OUTPUT=$($FOREST release annotate \
  --context-title "test release v1.0.0" \
  --context-description "Testing the notification system" \
  --metadata "version=1.0.0" \
  --source-username "${USERNAME}" \
  --source-email "${EMAIL}" \
  --commit-sha "abc1234567890def" \
  --commit-branch "main" \
  -o "${ORG}" \
  --project-name "${PROJECT}" 2>&1)
cd "${REPO_ROOT}"

SLUG=$(echo "${ANNOTATE_OUTPUT}" | grep "published artifact:" | awk '{print $3}')
if [ -z "${SLUG}" ]; then
  echo "ERROR: Failed to extract slug."
  echo "Output: ${ANNOTATE_OUTPUT}"
  kill "${LISTEN_PID}" 2>/dev/null || true
  exit 1
fi
echo "Artifact annotated: ${SLUG}"
sleep 3
echo ""

# ── Step 6: Release with --wait ──────────────────────────────────
echo "--- Step 6: Release artifact (with --wait) ---"
$FOREST release "${SLUG}" --environment "dev" --wait 2>&1 | grep -E "Release completed|Release failed|Error" || true
echo ""

# Wait for notifications to propagate
sleep 6

# ── Step 7: Show captured notifications ──────────────────────────
echo "--- Step 7: Notifications captured by listener ---"
echo ""
grep -E "^\[" "${NOTIFICATION_LOG}" -A3 || echo "(none captured)"
echo ""

# ── Step 8: List notifications via API ───────────────────────────
echo "--- Step 8: List all notifications ---"
$FOREST notifications list --limit 20 2>&1 | grep -E "^\[|^  " | head -20
echo ""

# ── Cleanup ──────────────────────────────────────────────────────
kill "${LISTEN_PID}" 2>/dev/null || true
wait "${LISTEN_PID}" 2>/dev/null || true

echo "=== Test complete ==="
echo ""
echo "Expected notifications:"
echo "  1. [ANNOTATED] Artifact annotated: ${SLUG}"
echo "  2. [STARTED]   Release started: ${ORG}/${PROJECT}"
echo "  3. [SUCCEEDED] Release succeeded: ${ORG}/${PROJECT}"
