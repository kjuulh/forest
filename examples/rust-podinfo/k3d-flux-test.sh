#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# rust-podinfo k3d + Flux v2 end-to-end integration test
#
# Proves the forest/flux@1 destination works with a real Kubernetes cluster:
#   1. Creates a k3d registry + cluster
#   2. Builds and pushes rust-podinfo image to the local registry
#   3. Starts a Gitea git server on the k3d network
#   4. Runs the forest release pipeline (prepare → annotate → release)
#   5. Installs Flux v2 and configures it to watch the gitops repo
#   6. Waits for Flux reconciliation and verifies the deployment is running
#   7. Port-forwards and tests the HTTP endpoints
#
# Prerequisites:
#   - k3d, flux, kubectl, docker, mise installed
#   - forest server running (mise run develop)
#   - organisation "rawpotion" created:
#       mise run forest -- organisation create --name rawpotion
#   - kernel modules loaded (k3d shares host kernel):
#       sudo modprobe xt_multiport vxlan
#
# Usage:
#   cd examples/rust-podinfo
#   ./k3d-flux-test.sh
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PASS=0
FAIL=0
SKIP=0
PF_PID=""

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP + 1)); }

section() { echo ""; echo "=== $1 ==="; }

# Names
CLUSTER_NAME="forest-flux-test"
REGISTRY_NAME="forest-test-registry"
REGISTRY_HOST="k3d-${REGISTRY_NAME}.localhost"
REGISTRY_PORT=5111
GITEA_CONTAINER="forest-gitea"
GITEA_HOST_PORT=3333
GITEA_USER="forest"
GITEA_PASS="foresttest1"
GITEA_REPO="gitops"
DEST_NAME="flux-dev-k3d"

IMAGE_NAME="${REGISTRY_HOST}:${REGISTRY_PORT}/rust-podinfo:test"

TEST_DIR="$SCRIPT_DIR/.k3d-flux-test"

# --------------------------------------------------------------------------
# Cleanup handler
# --------------------------------------------------------------------------

CLEANED_UP=false
cleanup() {
  if [ "$CLEANED_UP" = true ]; then return; fi
  CLEANED_UP=true
  set +e  # Don't exit on errors during cleanup

  echo ""
  echo "=== Cleanup ==="

  # Kill port-forward
  if [ -n "$PF_PID" ]; then
    kill "$PF_PID" 2>/dev/null || true
  fi

  # Delete forest destination
  mise run forest -- destination delete --name "$DEST_NAME" > /dev/null 2>&1 || true

  # Delete k3d cluster + registry
  k3d cluster delete "$CLUSTER_NAME" 2>/dev/null || true
  k3d registry delete "$REGISTRY_NAME" 2>/dev/null || true

  # Remove gitea container
  docker rm -f "$GITEA_CONTAINER" 2>/dev/null || true

  # Clean build artifacts
  rm -rf "$TEST_DIR" .forest
}
trap cleanup EXIT

# --------------------------------------------------------------------------
section "0. Prerequisites"
# --------------------------------------------------------------------------

PREREQ_OK=true
for cmd in k3d flux kubectl docker mise; do
  if command -v "$cmd" &>/dev/null; then
    pass "prerequisite: $cmd"
  else
    fail "prerequisite: $cmd not found"
    PREREQ_OK=false
  fi
done

if [ "$PREREQ_OK" != true ]; then
  echo "  Missing prerequisites, aborting."
  exit 1
fi

# k3d shares the host kernel — verify required modules are loaded
for mod in xt_multiport vxlan; do
  if grep -qw "$mod" /proc/modules; then
    pass "kernel module: $mod"
  else
    fail "kernel module: $mod not loaded (run: sudo modprobe $mod)"
    PREREQ_OK=false
  fi
done

if [ "$PREREQ_OK" != true ]; then
  echo "  Missing kernel modules, aborting."
  exit 1
fi

# Clean previous test artifacts
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR"

# --------------------------------------------------------------------------
section "1. Create k3d registry + cluster"
# --------------------------------------------------------------------------

# Clean up any leftover resources from a previous failed run
k3d cluster delete "$CLUSTER_NAME" 2>/dev/null || true
k3d registry delete "$REGISTRY_NAME" 2>/dev/null || true
docker rm -f "$GITEA_CONTAINER" 2>/dev/null || true

echo "  Creating k3d registry: $REGISTRY_NAME (port $REGISTRY_PORT)"
if k3d registry create "$REGISTRY_NAME" --port "$REGISTRY_PORT" 2>&1 | tail -1; then
  pass "k3d registry created"
else
  fail "k3d registry create"
  exit 1
fi

echo "  Creating k3d cluster: $CLUSTER_NAME"
REGISTRY_CONTAINER="k3d-${REGISTRY_NAME}"
if k3d cluster create "$CLUSTER_NAME" \
  --registry-use "${REGISTRY_CONTAINER}:${REGISTRY_PORT}" \
  --servers 1 \
  --agents 0 \
  --api-port 6550 \
  --wait \
  --timeout 120s 2>&1 | tail -3; then
  pass "k3d cluster created"
else
  fail "k3d cluster create"
  exit 1
fi

# Wait for API server to be reachable
echo "  Waiting for Kubernetes API server..."
K8S_READY=false
for i in $(seq 1 60); do
  if kubectl cluster-info > /dev/null 2>&1; then
    K8S_READY=true
    break
  fi
  sleep 2
done

if [ "$K8S_READY" = true ]; then
  pass "kubectl connected to cluster"
else
  fail "kubectl not connected after 120s"
  exit 1
fi

# Wait for node to be Ready
echo "  Waiting for node to be Ready..."
if kubectl wait --for=condition=Ready node --all --timeout=120s > /dev/null 2>&1; then
  pass "node is Ready"
else
  fail "node not Ready"
fi

# --------------------------------------------------------------------------
section "2. Build and push rust-podinfo image"
# --------------------------------------------------------------------------

# Use forest to compile the binary, then package into a minimal Docker image
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$REPO_ROOT/target/release/rust-podinfo"

echo "  Compiling rust-podinfo (forest run compile)"
if mise run forest -- run compile 2>&1 | tail -3; then
  pass "forest run compile"
else
  fail "forest run compile"
  exit 1
fi

DOCKERFILE="$TEST_DIR/Dockerfile"
cat > "$DOCKERFILE" <<DOCKERFILE_EOF
FROM debian:bookworm-slim
COPY rust-podinfo /usr/local/bin/rust-podinfo
RUN rust-podinfo --help
CMD ["rust-podinfo"]
DOCKERFILE_EOF

# Copy binary next to Dockerfile for the build context
cp "$BINARY" "$TEST_DIR/rust-podinfo"

echo "  Building image: $IMAGE_NAME"
if docker build -t "$IMAGE_NAME" -f "$DOCKERFILE" "$TEST_DIR" 2>&1 | tail -5; then
  pass "docker build"
else
  fail "docker build"
  exit 1
fi

echo "  Pushing image to k3d registry"
if docker push "$IMAGE_NAME" 2>&1 | tail -3; then
  pass "docker push to k3d registry"
else
  fail "docker push"
  exit 1
fi

# --------------------------------------------------------------------------
section "3. Start Gitea git server"
# --------------------------------------------------------------------------

K3D_NETWORK="k3d-${CLUSTER_NAME}"

echo "  Starting Gitea on network $K3D_NETWORK"
if docker run -d \
  --name "$GITEA_CONTAINER" \
  -e GITEA__security__INSTALL_LOCK=true \
  -e GITEA__service__DISABLE_REGISTRATION=false \
  -e "GITEA__server__ROOT_URL=http://${GITEA_CONTAINER}:3000" \
  --network "$K3D_NETWORK" \
  -p "${GITEA_HOST_PORT}:3000" \
  gitea/gitea:latest-rootless > /dev/null 2>&1; then
  pass "gitea container started"
else
  fail "gitea container start"
  exit 1
fi

# Wait for Gitea to be ready
echo "  Waiting for Gitea to be ready..."
GITEA_READY=false
for i in $(seq 1 60); do
  if curl -sf "http://localhost:${GITEA_HOST_PORT}/api/v1/version" > /dev/null 2>&1; then
    GITEA_READY=true
    break
  fi
  sleep 1
done

if [ "$GITEA_READY" = true ]; then
  pass "gitea is ready"
else
  fail "gitea not ready after 60s"
  exit 1
fi

# Create admin user via gitea CLI inside the container
echo "  Creating Gitea user: $GITEA_USER"
if docker exec "$GITEA_CONTAINER" \
  gitea admin user create \
  --username "$GITEA_USER" \
  --password "$GITEA_PASS" \
  --email "forest@test.io" \
  --admin \
  --must-change-password=false > /dev/null 2>&1; then
  pass "gitea user created"
else
  fail "gitea user create"
  exit 1
fi

# Create repo via API (now using the admin user)
echo "  Creating Gitea repo: $GITEA_REPO"
CREATE_REPO_RESP=$(curl -sf -X POST "http://localhost:${GITEA_HOST_PORT}/api/v1/user/repos" \
  -u "${GITEA_USER}:${GITEA_PASS}" \
  -H "Content-Type: application/json" \
  -d "{
    \"name\": \"${GITEA_REPO}\",
    \"default_branch\": \"main\",
    \"auto_init\": true
  }" 2>&1) || true

if echo "$CREATE_REPO_RESP" | grep -q "\"name\":\"${GITEA_REPO}\""; then
  pass "gitea repo created"
else
  fail "gitea repo create: $CREATE_REPO_RESP"
  exit 1
fi

GITEA_GIT_URL="http://${GITEA_USER}:${GITEA_PASS}@localhost:${GITEA_HOST_PORT}/${GITEA_USER}/${GITEA_REPO}.git"
GITEA_FLUX_URL="http://${GITEA_CONTAINER}:3000/${GITEA_USER}/${GITEA_REPO}.git"

echo "  Git URL (host):    $GITEA_GIT_URL"
echo "  Git URL (cluster): $GITEA_FLUX_URL"

# --------------------------------------------------------------------------
section "4. Forest release pipeline"
# --------------------------------------------------------------------------

# Delete old destinations (ours + any from flux-test.sh that could match)
mise run forest -- destination delete --name "$DEST_NAME" > /dev/null 2>&1 || true
for old_dest in flux-dev flux-staging flux-prod; do
  mise run forest -- destination delete --name "$old_dest" > /dev/null 2>&1 || true
done

# Create flux destination pointing at gitea
echo "  Creating destination: $DEST_NAME"
if mise run forest -- destination create \
  --organisation rawpotion \
  --name "$DEST_NAME" \
  --environment dev \
  --type "forest/flux@1" \
  --metadata "cluster_name=dev-cluster-01" \
  --metadata "namespace=rust-podinfo" \
  --metadata "git_url=${GITEA_GIT_URL}" \
  --metadata "git_branch=main" > /dev/null 2>&1; then
  pass "destination created: $DEST_NAME"
else
  fail "destination create: $DEST_NAME"
fi

# Release prepare
echo "  Running: forest release prepare"
if mise run forest -- release prepare > /dev/null 2>&1; then
  pass "release prepare"
else
  fail "release prepare"
fi

# Release annotate
echo "  Running: forest release annotate"
ANNOTATE_OUTPUT=$(mise run forest -- release annotate \
  --context-title "k3d flux integration test" \
  --context-description "Automated k3d + flux v2 e2e test" \
  --organisation rawpotion \
  --project-name rust-podinfo \
  --commit-sha "$(git rev-parse HEAD 2>/dev/null || echo test123)" \
  --commit-branch "$(git branch --show-current 2>/dev/null || echo main)" \
  --commit-message "test: k3d flux e2e" \
  --version 0.1.0 2>&1)

SLUG=$(echo "$ANNOTATE_OUTPUT" | grep "published artifact:" | sed 's/.*published artifact: //')

if [ -n "$SLUG" ]; then
  pass "release annotate (slug: $SLUG)"
else
  fail "release annotate (no slug)"
  echo "  Output: $ANNOTATE_OUTPUT" | tail -5
fi

# Release to dev
if [ -n "$SLUG" ]; then
  echo "  Releasing to dev..."
  if mise run forest -- release "$SLUG" --environment dev 2>&1 | grep -q "Release completed successfully"; then
    pass "release to dev"
  else
    fail "release to dev"
  fi
else
  skip "release to dev (no slug)"
fi

# Verify gitea repo has the release commit (clone and count)
RELEASE_PUSHED=false
for i in $(seq 1 10); do
  GITEA_COMMITS=$(git -C "$TEST_DIR" clone --bare --quiet "$GITEA_GIT_URL" gitops-check 2>/dev/null \
    && git -C "$TEST_DIR/gitops-check" rev-list --count HEAD 2>/dev/null || echo 0)
  rm -rf "$TEST_DIR/gitops-check"
  if [ "$GITEA_COMMITS" -ge 2 ]; then
    RELEASE_PUSHED=true
    break
  fi
  sleep 2
done

if [ "$RELEASE_PUSHED" = true ]; then
  pass "gitea repo has $GITEA_COMMITS commits (init + release)"
else
  fail "gitea repo expected >= 2 commits, got $GITEA_COMMITS"
fi

# --------------------------------------------------------------------------
section "5. Install Flux and configure sources"
# --------------------------------------------------------------------------

# Verify k8s API is still reachable
echo "  Checking Kubernetes API is reachable..."
K8S_STILL_OK=false
for i in $(seq 1 15); do
  if kubectl cluster-info > /dev/null 2>&1; then
    K8S_STILL_OK=true
    break
  fi
  echo "    Waiting for API server... (attempt $i)"
  sleep 2
done

if [ "$K8S_STILL_OK" = true ]; then
  pass "kubernetes API still reachable"
else
  fail "kubernetes API unreachable before flux install"
  kubectl cluster-info 2>&1 | sed 's/^/    /' || true
  exit 1
fi

echo "  Installing Flux v2 (source-controller + kustomize-controller)"
if flux install --components=source-controller,kustomize-controller 2>&1 | tail -3; then
  pass "flux install"
else
  fail "flux install"
  exit 1
fi

# Wait for flux to be ready
echo "  Waiting for Flux controllers..."
if kubectl -n flux-system wait deployment/source-controller \
  --for=condition=Available --timeout=120s > /dev/null 2>&1; then
  pass "source-controller ready"
else
  fail "source-controller not ready"
fi

if kubectl -n flux-system wait deployment/kustomize-controller \
  --for=condition=Available --timeout=120s > /dev/null 2>&1; then
  pass "kustomize-controller ready"
else
  fail "kustomize-controller not ready"
fi

# Create gitea credentials secret
echo "  Creating gitea credentials secret"
if kubectl -n flux-system create secret generic gitea-creds \
  --from-literal=username="${GITEA_USER}" \
  --from-literal=password="${GITEA_PASS}" > /dev/null 2>&1; then
  pass "gitea-creds secret created"
else
  fail "gitea-creds secret"
fi

# Create GitRepository source (named "flux-system" to match generated CRs)
echo "  Creating GitRepository source: flux-system"
if flux create source git flux-system \
  --url="${GITEA_FLUX_URL}" \
  --branch=main \
  --interval=30s \
  --secret-ref=gitea-creds 2>&1 | tail -3; then
  pass "GitRepository flux-system created"
else
  fail "GitRepository create"
fi

# Create root Kustomization to apply the clusters/ path
CLUSTERS_PATH="./clusters/dev/${DEST_NAME}/dev-cluster-01/rust-podinfo"
echo "  Creating root Kustomization (path: $CLUSTERS_PATH)"
if flux create kustomization gitops-root \
  --source=GitRepository/flux-system \
  --path="$CLUSTERS_PATH" \
  --prune=true \
  --interval=1m 2>&1 | tail -3; then
  pass "Kustomization gitops-root created"
else
  fail "Kustomization create"
fi

# --------------------------------------------------------------------------
section "6. Wait for Flux reconciliation"
# --------------------------------------------------------------------------

# Trigger initial reconciliation
echo "  Triggering reconciliation..."
flux reconcile source git flux-system 2>/dev/null || true
sleep 5

# Wait for root kustomization
echo "  Waiting for gitops-root kustomization..."
if kubectl -n flux-system wait kustomization/gitops-root \
  --for=condition=Ready --timeout=180s > /dev/null 2>&1; then
  pass "kustomization gitops-root is Ready"
else
  fail "kustomization gitops-root not Ready"
  echo "  Status:"
  flux get kustomization gitops-root 2>&1 | sed 's/^/    /'
fi

# Wait for the nested kustomization (the one generated by forest)
echo "  Waiting for rawpotion-rust-podinfo kustomization..."
if kubectl -n flux-system wait kustomization/rawpotion-rust-podinfo \
  --for=condition=Ready --timeout=180s > /dev/null 2>&1; then
  pass "kustomization rawpotion-rust-podinfo is Ready"
else
  fail "kustomization rawpotion-rust-podinfo not Ready"
  echo "  Status:"
  flux get kustomization --all-namespaces 2>&1 | sed 's/^/    /'
fi

# Wait for the deployment
echo "  Waiting for deployment rust-podinfo..."
if kubectl -n rust-podinfo wait deployment/rust-podinfo \
  --for=condition=Available --timeout=120s > /dev/null 2>&1; then
  pass "deployment rust-podinfo is Available"
else
  fail "deployment rust-podinfo not Available"
  echo "  Pods:"
  kubectl -n rust-podinfo get pods 2>&1 | sed 's/^/    /'
  echo "  Events:"
  kubectl -n rust-podinfo get events --sort-by='.lastTimestamp' 2>&1 | tail -10 | sed 's/^/    /'
fi

# Check replica count
POD_COUNT=$(kubectl -n rust-podinfo get pods -l app.kubernetes.io/name=rust-podinfo \
  --field-selector=status.phase=Running --no-headers 2>/dev/null | wc -l)
if [ "$POD_COUNT" -eq 1 ]; then
  pass "pod count: $POD_COUNT (matches dev replicas=1)"
else
  fail "pod count: expected 1, got $POD_COUNT"
fi

# --------------------------------------------------------------------------
section "7. Test HTTP endpoints via port-forward"
# --------------------------------------------------------------------------

echo "  Starting port-forward..."
kubectl -n rust-podinfo port-forward svc/rust-podinfo 18080:8080 > /dev/null 2>&1 &
PF_PID=$!
sleep 3

# Test root endpoint
INFO_RESP=$(curl -sf http://localhost:18080/ 2>&1) || true
if echo "$INFO_RESP" | grep -q "rust-podinfo"; then
  pass "GET / returns rust-podinfo info"
  echo "  Response: $INFO_RESP"
else
  fail "GET / did not return expected response"
  echo "  Response: $INFO_RESP"
fi

# Test version endpoint
VERSION_RESP=$(curl -sf http://localhost:18080/version 2>&1) || true
if echo "$VERSION_RESP" | grep -q "version"; then
  pass "GET /version returns version"
else
  fail "GET /version did not return expected response"
fi

# Test env endpoint
ENV_RESP=$(curl -sf http://localhost:18080/env 2>&1) || true
if echo "$ENV_RESP" | grep -q "PODINFO_ENV"; then
  pass "GET /env returns PODINFO_ENV"
else
  # Env vars depend on deployment config, just check it responds
  if echo "$ENV_RESP" | grep -q "env"; then
    pass "GET /env responds (PODINFO_ENV may not be set)"
  else
    fail "GET /env did not respond"
  fi
fi

kill "$PF_PID" 2>/dev/null || true
PF_PID=""

# --------------------------------------------------------------------------
section "8. Flux status summary"
# --------------------------------------------------------------------------

echo ""
echo "  --- Flux sources ---"
flux get sources git 2>&1 | sed 's/^/    /'

echo ""
echo "  --- Flux kustomizations ---"
flux get kustomizations --all-namespaces 2>&1 | sed 's/^/    /'

echo ""
echo "  --- Deployment ---"
kubectl -n rust-podinfo get deployment,pods,svc 2>&1 | sed 's/^/    /'

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
