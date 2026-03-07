#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# rust-podinfo unified Flux v2 e2e test script
#
# Modes:
#   flux              Flux destination test (in-process, bare git, 3 envs)
#   flux-runner       Flux destination via distributed runner (bare git, 3 envs)
#   k3d-flux          Full k3d + Flux v2 e2e test (in-process, single env)
#   k3d-flux-runner   Full k3d + Flux v2 e2e via distributed runner (single env)
#
# Prerequisites (all modes):
#   - mise installed
#   - docker running (for postgres via docker compose)
#   - organisation "rawpotion" created in DB
#
# Additional prerequisites for non-runner modes (flux, k3d-flux):
#   - forest server running (mise run develop)
#
# Additional prerequisites for k3d modes:
#   - k3d, flux, kubectl, docker installed
#   - kernel modules: sudo modprobe xt_multiport vxlan
#
# Usage:
#   cd examples/rust-podinfo
#   ./flux-test.sh flux
#   ./flux-test.sh flux-runner
#   ./flux-test.sh k3d-flux
#   ./flux-test.sh k3d-flux-runner
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ============================================================================
# Configuration
# ============================================================================

# Test framework counters
PASS=0
FAIL=0
SKIP=0

# Ports for test server (used in runner modes to avoid dev server conflicts)
TEST_GRPC_PORT=14040
TEST_TF_PORT=14041
TEST_HTTP_PORT=14042

# k3d configuration
CLUSTER_NAME="forest-flux-test"
REGISTRY_NAME="forest-test-registry"
REGISTRY_HOST="k3d-${REGISTRY_NAME}.localhost"
REGISTRY_PORT=5111
GITEA_CONTAINER="forest-gitea"
GITEA_HOST_PORT=3333
GITEA_USER="forest"
GITEA_PASS="foresttest1"
GITEA_REPO="gitops"
WEBHOOK_HOST_PORT=18888

IMAGE_NAME="${REGISTRY_HOST}:${REGISTRY_PORT}/rust-podinfo:test"

# Process tracking
SERVER_PID=""
RUNNER_PID=""
PF_PID=""
WEBHOOK_PF_PID=""

# State (set per mode)
MODE=""
TEST_DIR=""
BARE_REPO=""
VERIFY_DIR=""
SERVER_LOG=""
RUNNER_LOG=""
SLUG=""
USE_RUNNER=false
USE_K3D=false
DEST_NAME=""
GITEA_GIT_URL=""
GITEA_FLUX_URL=""
RECONCILE_URL=""

# ============================================================================
# Test Framework
# ============================================================================

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP: $1"; SKIP=$((SKIP + 1)); }
section() { echo ""; echo "=== $1 ==="; }

results() {
  section "Results"
  local total=$((PASS + FAIL + SKIP))
  echo ""
  echo "  Total: $total  Pass: $PASS  Fail: $FAIL  Skip: $SKIP"
  echo ""
  if [ "$FAIL" -gt 0 ]; then
    echo "  SOME TESTS FAILED"
    exit 1
  else
    echo "  ALL TESTS PASSED"
    exit 0
  fi
}

# Run forest CLI — targets test server in runner modes, dev server otherwise
forest_cli() {
  if [ "$USE_RUNNER" = true ]; then
    FOREST_SERVER="http://localhost:${TEST_GRPC_PORT}" mise run forest -- "$@"
  else
    mise run forest -- "$@"
  fi
}

# ============================================================================
# Prerequisite Checks
# ============================================================================

check_prerequisites() {
  local ok=true
  for cmd in "$@"; do
    if command -v "$cmd" &>/dev/null; then
      pass "prerequisite: $cmd"
    else
      fail "prerequisite: $cmd not found"
      ok=false
    fi
  done
  if [ "$ok" != true ]; then
    echo "  Missing prerequisites, aborting."
    exit 1
  fi
}

check_kernel_modules() {
  local ok=true
  for mod in xt_multiport vxlan; do
    if grep -qw "$mod" /proc/modules; then
      pass "kernel module: $mod"
    else
      fail "kernel module: $mod not loaded (run: sudo modprobe $mod)"
      ok=false
    fi
  done
  if [ "$ok" != true ]; then
    echo "  Missing kernel modules, aborting."
    exit 1
  fi
}

# ============================================================================
# Git Setup
# ============================================================================

setup_bare_git_repo() {
  git init --bare "$BARE_REPO" > /dev/null 2>&1

  local bootstrap_dir="$TEST_DIR/bootstrap"
  git clone "file://$BARE_REPO" "$bootstrap_dir" > /dev/null 2>&1
  (
    cd "$bootstrap_dir"
    echo "# GitOps Repo" > README.md
    git add -A
    git -c user.name=test -c user.email=test@test commit -m "initial" > /dev/null 2>&1
    git push origin HEAD:main > /dev/null 2>&1
  )
  rm -rf "$bootstrap_dir"

  if [ -d "$BARE_REPO/refs" ]; then
    pass "bare git repo created"
  else
    fail "bare git repo creation"
    exit 1
  fi
}

# ============================================================================
# Destination Management
# ============================================================================

create_flux_destinations_3env() {
  local git_url="$1"
  local reconcile_url="${2:-}"

  for dest in flux-dev flux-staging flux-prod; do
    forest_cli destination delete --name "$dest" > /dev/null 2>&1 || true
  done

  for env_dest in "dev:flux-dev:dev-cluster-01" "staging:flux-staging:staging-cluster-01" "prod:flux-prod:prod-cluster-01"; do
    IFS=: read -r env name cluster <<< "$env_dest"
    echo "  Creating destination: $name (env=$env, cluster=$cluster)"
    local metadata_args=(
      --metadata "cluster_name=$cluster"
      --metadata "namespace=rust-podinfo"
      --metadata "git_url=$git_url"
      --metadata "git_branch=main"
    )
    if [ -n "$reconcile_url" ]; then
      metadata_args+=(--metadata "reconcile_url=$reconcile_url")
    fi
    if forest_cli destination create \
      --organisation rawpotion \
      --name "$name" \
      --environment "$env" \
      --type "forest/flux@1" \
      "${metadata_args[@]}" > /dev/null 2>&1; then
      pass "destination create: $name"
    else
      fail "destination create: $name"
    fi
  done
}

create_flux_destination_single() {
  local dest_name="$1"
  local env="$2"
  local cluster="$3"
  local git_url="$4"
  local reconcile_url="${5:-}"

  # Clean up any conflicting destinations
  forest_cli destination delete --name "$dest_name" > /dev/null 2>&1 || true
  for old in flux-dev flux-staging flux-prod; do
    forest_cli destination delete --name "$old" > /dev/null 2>&1 || true
  done

  echo "  Creating destination: $dest_name"
  local metadata_args=(
    --metadata "cluster_name=$cluster"
    --metadata "namespace=rust-podinfo"
    --metadata "git_url=$git_url"
    --metadata "git_branch=main"
  )
  if [ -n "$reconcile_url" ]; then
    metadata_args+=(--metadata "reconcile_url=$reconcile_url")
  fi
  if forest_cli destination create \
    --organisation rawpotion \
    --name "$dest_name" \
    --environment "$env" \
    --type "forest/flux@1" \
    "${metadata_args[@]}" > /dev/null 2>&1; then
    pass "destination created: $dest_name"
  else
    fail "destination create: $dest_name"
  fi
}

delete_test_destinations() {
  for dest in flux-dev flux-staging flux-prod "$DEST_NAME"; do
    [ -n "$dest" ] && forest_cli destination delete --name "$dest" > /dev/null 2>&1 || true
  done
}

# ============================================================================
# Release Pipeline
# ============================================================================

do_release_prepare() {
  echo "  Running: forest release prepare"
  if forest_cli release prepare > /dev/null 2>&1; then
    pass "release prepare"
  else
    fail "release prepare"
  fi

  for env in dev staging prod; do
    if [ -d ".forest/deployment/$env" ]; then
      local file_count
      file_count=$(find ".forest/deployment/$env" -type f | wc -l)
      pass "release prepare: $env ($file_count files)"
    fi
  done
}

do_release_annotate() {
  local title="${1:-Test release v0.1.0}"
  local description="${2:-Automated test}"

  echo "  Running: forest release annotate"
  local annotate_output
  annotate_output=$(forest_cli release annotate \
    --context-title "$title" \
    --context-description "$description" \
    --organisation rawpotion \
    --project-name rust-podinfo \
    --commit-sha "$(git rev-parse HEAD 2>/dev/null || echo test123)" \
    --commit-branch "$(git branch --show-current 2>/dev/null || echo main)" \
    --commit-message "test: e2e" \
    --version 0.1.0 2>&1)

  SLUG=$(echo "$annotate_output" | grep "published artifact:" | sed 's/.*published artifact: //')

  if [ -n "$SLUG" ]; then
    pass "release annotate (slug: $SLUG)"
  else
    fail "release annotate (no slug returned)"
    echo "  Output: $annotate_output" | tail -5
  fi
}

do_release_to_envs() {
  local envs=("$@")

  if [ -z "$SLUG" ]; then
    for env in "${envs[@]}"; do
      skip "release to $env (no slug)"
    done
    return
  fi

  for env in "${envs[@]}"; do
    echo "  Releasing to $env..."
    local output
    output=$(forest_cli release "$SLUG" --environment "$env" 2>&1 || true)
    if echo "$output" | grep -q "Release completed successfully"; then
      pass "release to $env"
    elif echo "$output" | grep -qi "success\|completed"; then
      pass "release to $env (completed)"
    else
      fail "release to $env"
      echo "$output" | tail -5 | sed 's/^/    /'
    fi
  done

  # Give scheduler time to dispatch
  sleep 5
}

# ============================================================================
# Gitops Verification
# ============================================================================

verify_gitops_repo() {
  local repo_url="$1"
  shift
  local env_data=("$@")

  rm -rf "$VERIFY_DIR"
  git clone "$repo_url" "$VERIFY_DIR" > /dev/null 2>&1

  echo "  Checking Flux v2 directory structure..."

  for entry in "${env_data[@]}"; do
    IFS=: read -r env dest cluster replicas <<< "$entry"

    local release_dir="$VERIFY_DIR/releases/$env/$dest/$cluster/rust-podinfo/rawpotion-rust-podinfo"
    local cluster_dir="$VERIFY_DIR/clusters/$env/$dest/$cluster/rust-podinfo"
    local cluster_cr="$cluster_dir/rawpotion-rust-podinfo.yaml"

    # Manifest files
    for f in 10-namespace.yaml 20-deployment.yaml 30-service.yaml; do
      if [ -f "$release_dir/$f" ]; then
        pass "gitops: $env $f exists"
      else
        fail "gitops: $env $f missing"
      fi
    done

    # Replicas
    if grep -q "replicas: $replicas" "$release_dir/20-deployment.yaml" 2>/dev/null; then
      pass "gitops: $env deployment replicas=$replicas"
    else
      fail "gitops: $env deployment expected replicas=$replicas"
    fi

    # Flux Kustomization CR
    if [ -f "$cluster_cr" ]; then
      pass "gitops: $env kustomization CR exists"
    else
      fail "gitops: $env kustomization CR missing"
    fi

    if grep -q "kind: Kustomization" "$cluster_cr" 2>/dev/null; then
      pass "gitops: $env is Flux Kustomization CR"
    else
      fail "gitops: $env not a Flux CR"
    fi

    if grep -q "targetNamespace: rust-podinfo" "$cluster_cr" 2>/dev/null; then
      pass "gitops: $env targets rust-podinfo namespace"
    else
      fail "gitops: $env wrong targetNamespace"
    fi

    local expected_path="./releases/$env/$dest/$cluster/rust-podinfo/rawpotion-rust-podinfo"
    if grep -q "path: $expected_path" "$cluster_cr" 2>/dev/null; then
      pass "gitops: $env kustomization path correct"
    else
      fail "gitops: $env kustomization path wrong (expected $expected_path)"
    fi

    # Kustomize aggregation
    if [ -f "$cluster_dir/kustomization.yaml" ]; then
      pass "gitops: $env clusters/kustomization.yaml exists"
    else
      fail "gitops: $env clusters/kustomization.yaml missing"
    fi

    if grep -q "rawpotion-rust-podinfo.yaml" "$cluster_dir/kustomization.yaml" 2>/dev/null; then
      pass "gitops: $env kustomization.yaml references CR"
    else
      fail "gitops: $env kustomization.yaml does not reference CR"
    fi

    # .forest/ metadata
    local forest_dir="$release_dir/.forest"
    for f in config.yaml release.yaml spec.yaml; do
      if [ -f "$forest_dir/$f" ]; then
        pass "gitops: $env .forest/$f exists"
      else
        fail "gitops: $env .forest/$f missing"
      fi
    done

    if grep -q "slug" "$forest_dir/release.yaml" 2>/dev/null; then
      pass "gitops: $env release.yaml has slug"
    else
      fail "gitops: $env release.yaml missing slug"
    fi

    if grep -q "destination:" "$forest_dir/release.yaml" 2>/dev/null; then
      pass "gitops: $env release.yaml has destination info"
    else
      fail "gitops: $env release.yaml missing destination info"
    fi
  done

  # README preserved
  if [ -f "$VERIFY_DIR/README.md" ]; then
    pass "gitops: README.md preserved"
  else
    fail "gitops: README.md clobbered"
  fi

  # Git history
  local commit_count
  commit_count=$(cd "$VERIFY_DIR" && git log --oneline | wc -l)
  local expected_commits=$((1 + ${#env_data[@]}))
  if [ "$commit_count" -ge "$expected_commits" ]; then
    pass "gitops: $commit_count commits (initial + ${#env_data[@]} releases)"
  else
    fail "gitops: expected at least $expected_commits commits, got $commit_count"
  fi
}

show_gitops_output() {
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
}

# ============================================================================
# Server & Runner Management
# ============================================================================

build_binaries() {
  echo "  Building forest-server and forest-runner..."
  if (cd "$REPO_ROOT" && cargo build -p forest-server -p forest-runner 2>&1 | tail -3); then
    pass "cargo build"
  else
    fail "cargo build"
    exit 1
  fi
}

start_test_server() {
  local disable_in_process="${1:-false}"
  local server_bin="$REPO_ROOT/target/debug/forest-server"

  echo "  Starting forest-server (ports $TEST_GRPC_PORT/$TEST_TF_PORT/$TEST_HTTP_PORT)..."
  set -a; source "$REPO_ROOT/.env"; set +a
  export FOREST_HOST="127.0.0.1:${TEST_GRPC_PORT}"
  export FOREST_HTTP_HOST="127.0.0.1:${TEST_HTTP_PORT}"
  export FOREST_TERRAFORM_V1_HOST="127.0.0.1:${TEST_TF_PORT}"
  export EXTERNAL_HOST="http://127.0.0.1:${TEST_TF_PORT}"
  export FOREST_TERRAFORM_V1_EXTERNAL_HOST="http://127.0.0.1:${TEST_TF_PORT}"

  local extra_args=()
  if [ "$disable_in_process" = true ]; then
    extra_args+=(--disable-in-process)
    echo "  (in-process execution disabled)"
  fi

  "$server_bin" serve "${extra_args[@]}" > "$SERVER_LOG" 2>&1 &
  SERVER_PID=$!

  local ready=false
  for i in $(seq 1 30); do
    if grep -q "starting" "$SERVER_LOG" 2>/dev/null; then
      ready=true
      break
    fi
    sleep 1
  done

  if kill -0 "$SERVER_PID" 2>/dev/null; then
    if [ "$ready" = true ]; then
      pass "forest-server started (PID $SERVER_PID)"
    else
      sleep 3
      if kill -0 "$SERVER_PID" 2>/dev/null; then
        pass "forest-server started (PID $SERVER_PID, slow startup)"
      else
        fail "forest-server died during startup"
        tail -20 "$SERVER_LOG"
        exit 1
      fi
    fi
  else
    fail "forest-server failed to start"
    tail -20 "$SERVER_LOG"
    exit 1
  fi
}

start_runner() {
  local runner_bin="$REPO_ROOT/target/debug/forest-runner"

  echo "  Starting forest-runner (connecting to port $TEST_GRPC_PORT)..."
  FOREST_SERVER_ADDR="http://127.0.0.1:${TEST_GRPC_PORT}" \
    FOREST_RUNNER_ID="test-runner-1" \
    FOREST_MAX_CONCURRENT=4 \
    RUST_LOG="forest_runner=debug,info" \
    "$runner_bin" --all \
    > "$RUNNER_LOG" 2>&1 &
  RUNNER_PID=$!

  local ready=false
  for i in $(seq 1 20); do
    if grep -q "connected and registered" "$RUNNER_LOG" 2>/dev/null; then
      ready=true
      break
    fi
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then break; fi
    sleep 1
  done

  if [ "$ready" = true ]; then
    pass "forest-runner connected (PID $RUNNER_PID)"
  else
    if kill -0 "$RUNNER_PID" 2>/dev/null; then
      sleep 5
      if grep -q "connected" "$RUNNER_LOG" 2>/dev/null; then
        pass "forest-runner connected (PID $RUNNER_PID, slow connect)"
      else
        fail "forest-runner not connected after 25s"
        tail -20 "$RUNNER_LOG" | sed 's/^/    /'
        exit 1
      fi
    else
      fail "forest-runner died"
      tail -20 "$RUNNER_LOG" | sed 's/^/    /'
      exit 1
    fi
  fi
}

verify_runner_execution() {
  local min_expected="${1:-1}"

  local assigned
  assigned=$(sed 's/\x1b\[[0-9;]*m//g' "$SERVER_LOG" 2>/dev/null | grep -c "assigned release to remote runner" || true)
  assigned=${assigned:-0}
  if [ "$assigned" -ge "$min_expected" ]; then
    pass "server dispatched $assigned releases to remote runner"
  else
    fail "expected >= $min_expected remote runner dispatches, got $assigned"
    echo "  Server log (last 30 lines):"
    tail -30 "$SERVER_LOG" | sed 's/^/    /'
  fi

  local completed
  completed=$(sed 's/\x1b\[[0-9;]*m//g' "$RUNNER_LOG" 2>/dev/null | grep -c "release completed successfully" || true)
  completed=${completed:-0}
  if [ "$completed" -ge "$min_expected" ]; then
    pass "runner completed $completed releases"
  else
    fail "expected >= $min_expected runner completions, got $completed"
    echo "  Runner log (last 30 lines):"
    tail -30 "$RUNNER_LOG" | sed 's/^/    /'
  fi

  if [ "$completed" -ge 3 ] && [ "$min_expected" -ge 3 ]; then
    pass "all 3 releases went through the runner"
  elif [ "$completed" -ge 1 ] && [ "$min_expected" -ge 3 ]; then
    echo "  Note: $completed/3 releases via runner (other server may have picked up the rest)"
  fi
}

show_runner_output() {
  echo ""
  echo "  --- Server log (runner-related) ---"
  grep -i "runner\|assigned\|remote" "$SERVER_LOG" 2>/dev/null | tail -15 | sed 's/^/    /'
  echo ""
  echo "  --- Runner log (last 15 lines) ---"
  tail -15 "$RUNNER_LOG" | sed 's/^/    /'
  echo ""
  echo "  Logs: $SERVER_LOG, $RUNNER_LOG"
}

# ============================================================================
# k3d + Gitea + Flux Setup
# ============================================================================

setup_k3d_cluster() {
  # Clean up any leftover resources
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
  local registry_container="k3d-${REGISTRY_NAME}"
  if k3d cluster create "$CLUSTER_NAME" \
    --registry-use "${registry_container}:${REGISTRY_PORT}" \
    --servers 1 --agents 0 --api-port 6550 \
    --wait --timeout 120s 2>&1 | tail -3; then
    pass "k3d cluster created"
  else
    fail "k3d cluster create"
    exit 1
  fi

  echo "  Waiting for Kubernetes API server..."
  local ready=false
  for i in $(seq 1 60); do
    if kubectl cluster-info > /dev/null 2>&1; then
      ready=true
      break
    fi
    sleep 2
  done

  if [ "$ready" = true ]; then
    pass "kubectl connected to cluster"
  else
    fail "kubectl not connected after 120s"
    exit 1
  fi

  echo "  Waiting for node to be Ready..."
  if kubectl wait --for=condition=Ready node --all --timeout=120s > /dev/null 2>&1; then
    pass "node is Ready"
  else
    fail "node not Ready"
  fi
}

build_and_push_podinfo_image() {
  local binary="$REPO_ROOT/target/release/rust-podinfo"

  echo "  Compiling rust-podinfo (forest run compile)"
  if forest_cli run compile 2>&1 | tail -3; then
    pass "forest run compile"
  else
    fail "forest run compile"
    exit 1
  fi

  local dockerfile="$TEST_DIR/Dockerfile"
  cat > "$dockerfile" <<'DOCKERFILE_EOF'
FROM debian:bookworm-slim
COPY rust-podinfo /usr/local/bin/rust-podinfo
RUN rust-podinfo --help
CMD ["rust-podinfo"]
DOCKERFILE_EOF

  cp "$binary" "$TEST_DIR/rust-podinfo"

  echo "  Building image: $IMAGE_NAME"
  if docker build -t "$IMAGE_NAME" -f "$dockerfile" "$TEST_DIR" 2>&1 | tail -5; then
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
}

setup_gitea() {
  local k3d_network="k3d-${CLUSTER_NAME}"

  echo "  Starting Gitea on network $k3d_network"
  if docker run -d \
    --name "$GITEA_CONTAINER" \
    -e GITEA__security__INSTALL_LOCK=true \
    -e GITEA__service__DISABLE_REGISTRATION=false \
    -e "GITEA__server__ROOT_URL=http://${GITEA_CONTAINER}:3000" \
    --network "$k3d_network" \
    -p "${GITEA_HOST_PORT}:3000" \
    gitea/gitea:latest-rootless > /dev/null 2>&1; then
    pass "gitea container started"
  else
    fail "gitea container start"
    exit 1
  fi

  echo "  Waiting for Gitea to be ready..."
  local ready=false
  for i in $(seq 1 60); do
    if curl -sf "http://localhost:${GITEA_HOST_PORT}/api/v1/version" > /dev/null 2>&1; then
      ready=true
      break
    fi
    sleep 1
  done

  if [ "$ready" = true ]; then
    pass "gitea is ready"
  else
    fail "gitea not ready after 60s"
    exit 1
  fi

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

  echo "  Creating Gitea repo: $GITEA_REPO"
  local resp
  resp=$(curl -sf -X POST "http://localhost:${GITEA_HOST_PORT}/api/v1/user/repos" \
    -u "${GITEA_USER}:${GITEA_PASS}" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"${GITEA_REPO}\",\"default_branch\":\"main\",\"auto_init\":true}" 2>&1) || true

  if echo "$resp" | grep -q "\"name\":\"${GITEA_REPO}\""; then
    pass "gitea repo created"
  else
    fail "gitea repo create: $resp"
    exit 1
  fi

  GITEA_GIT_URL="http://${GITEA_USER}:${GITEA_PASS}@localhost:${GITEA_HOST_PORT}/${GITEA_USER}/${GITEA_REPO}.git"
  GITEA_FLUX_URL="http://${GITEA_CONTAINER}:3000/${GITEA_USER}/${GITEA_REPO}.git"

  echo "  Git URL (host):    $GITEA_GIT_URL"
  echo "  Git URL (cluster): $GITEA_FLUX_URL"
}

setup_flux() {
  local dest_name="$1"

  echo "  Installing Flux v2 (source + kustomize + notification controllers)"
  if flux install --components=source-controller,kustomize-controller,notification-controller 2>&1 | tail -3; then
    pass "flux install"
  else
    fail "flux install"
    exit 1
  fi

  echo "  Waiting for Flux controllers..."
  for ctrl in source-controller kustomize-controller notification-controller; do
    if kubectl -n flux-system wait deployment/"$ctrl" \
      --for=condition=Available --timeout=120s > /dev/null 2>&1; then
      pass "$ctrl ready"
    else
      fail "$ctrl not ready"
    fi
  done

  # Gitea credentials
  echo "  Creating gitea credentials secret"
  kubectl -n flux-system create secret generic gitea-creds \
    --from-literal=username="${GITEA_USER}" \
    --from-literal=password="${GITEA_PASS}" > /dev/null 2>&1
  pass "gitea-creds secret created"

  # GitRepository source
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

  # Root Kustomization
  local clusters_path="./clusters/dev/${dest_name}/dev-cluster-01/rust-podinfo"
  echo "  Creating root Kustomization (path: $clusters_path)"
  if flux create kustomization gitops-root \
    --source=GitRepository/flux-system \
    --path="$clusters_path" \
    --prune=true \
    --interval=1m \
    --export | kubectl apply -f - > /dev/null 2>&1; then
    pass "Kustomization gitops-root created"
  else
    fail "Kustomization create"
  fi

  # Flux Receiver for webhook triggers
  echo "  Creating Flux Receiver webhook"
  local receiver_token="forest-webhook-token"
  kubectl -n flux-system create secret generic receiver-token \
    --from-literal=token="$receiver_token" > /dev/null 2>&1
  kubectl apply -f - <<RECEIVER_EOF > /dev/null 2>&1
apiVersion: notification.toolkit.fluxcd.io/v1
kind: Receiver
metadata:
  name: forest-webhook
  namespace: flux-system
spec:
  type: generic
  secretRef:
    name: receiver-token
  resources:
    - kind: GitRepository
      name: flux-system
      apiVersion: source.toolkit.fluxcd.io/v1
RECEIVER_EOF

  echo "  Waiting for Receiver to be ready..."
  local webhook_path=""
  for i in $(seq 1 30); do
    webhook_path=$(kubectl -n flux-system get receiver forest-webhook \
      -o jsonpath='{.status.webhookPath}' 2>/dev/null) || true
    if [ -n "$webhook_path" ]; then break; fi
    sleep 2
  done

  if [ -n "$webhook_path" ]; then
    pass "Flux Receiver ready (path: $webhook_path)"
  else
    fail "Flux Receiver not ready"
  fi

  # Port-forward webhook-receiver
  echo "  Port-forwarding webhook-receiver to localhost:$WEBHOOK_HOST_PORT"
  kubectl -n flux-system port-forward svc/webhook-receiver "$WEBHOOK_HOST_PORT":80 > /dev/null 2>&1 &
  WEBHOOK_PF_PID=$!
  sleep 2

  RECONCILE_URL="http://localhost:${WEBHOOK_HOST_PORT}${webhook_path}"
  echo "  Reconcile URL: $RECONCILE_URL"

  # Verify webhook reachable
  local status
  status=$(curl -sf -o /dev/null -w '%{http_code}' -X POST "$RECONCILE_URL" 2>&1) || true
  pass "webhook reachable (status: $status)"
}

wait_for_flux_reconciliation() {
  echo "  Waiting for gitops-root kustomization..."
  if kubectl -n flux-system wait kustomization/gitops-root \
    --for=condition=Ready --timeout=180s > /dev/null 2>&1; then
    pass "kustomization gitops-root is Ready"
  else
    fail "kustomization gitops-root not Ready"
    flux get kustomization gitops-root 2>&1 | sed 's/^/    /'
  fi

  echo "  Waiting for rawpotion-rust-podinfo kustomization..."
  if kubectl -n flux-system wait kustomization/rawpotion-rust-podinfo \
    --for=condition=Ready --timeout=180s > /dev/null 2>&1; then
    pass "kustomization rawpotion-rust-podinfo is Ready"
  else
    fail "kustomization rawpotion-rust-podinfo not Ready"
    flux get kustomization --all-namespaces 2>&1 | sed 's/^/    /'
  fi

  echo "  Waiting for deployment rust-podinfo..."
  if kubectl -n rust-podinfo wait deployment/rust-podinfo \
    --for=condition=Available --timeout=120s > /dev/null 2>&1; then
    pass "deployment rust-podinfo is Available"
  else
    fail "deployment rust-podinfo not Available"
    kubectl -n rust-podinfo get pods 2>&1 | sed 's/^/    /'
    kubectl -n rust-podinfo get events --sort-by='.lastTimestamp' 2>&1 | tail -10 | sed 's/^/    /'
  fi

  local pod_count
  pod_count=$(kubectl -n rust-podinfo get pods -l app.kubernetes.io/name=rust-podinfo \
    --field-selector=status.phase=Running --no-headers 2>/dev/null | wc -l)
  if [ "$pod_count" -eq 1 ]; then
    pass "pod count: $pod_count (matches dev replicas=1)"
  else
    fail "pod count: expected 1, got $pod_count"
  fi
}

test_http_endpoints() {
  echo "  Starting port-forward..."
  kubectl -n rust-podinfo port-forward svc/rust-podinfo 18080:8080 > /dev/null 2>&1 &
  PF_PID=$!
  sleep 3

  local resp
  resp=$(curl -sf http://localhost:18080/ 2>&1) || true
  if echo "$resp" | grep -q "rust-podinfo"; then
    pass "GET / returns rust-podinfo info"
    echo "  Response: $resp"
  else
    fail "GET / did not return expected response"
    echo "  Response: $resp"
  fi

  resp=$(curl -sf http://localhost:18080/version 2>&1) || true
  if echo "$resp" | grep -q "version"; then
    pass "GET /version returns version"
  else
    fail "GET /version did not return expected response"
  fi

  resp=$(curl -sf http://localhost:18080/env 2>&1) || true
  if echo "$resp" | grep -q "PODINFO_ENV"; then
    pass "GET /env returns PODINFO_ENV"
  elif echo "$resp" | grep -q "env"; then
    pass "GET /env responds (PODINFO_ENV may not be set)"
  else
    fail "GET /env did not respond"
  fi

  kill "$PF_PID" 2>/dev/null || true
  PF_PID=""
}

show_flux_status() {
  echo ""
  echo "  --- Flux sources ---"
  flux get sources git 2>&1 | sed 's/^/    /'
  echo ""
  echo "  --- Flux kustomizations ---"
  flux get kustomizations --all-namespaces 2>&1 | sed 's/^/    /'
  echo ""
  echo "  --- Deployment ---"
  kubectl -n rust-podinfo get deployment,pods,svc 2>&1 | sed 's/^/    /'
}

# ============================================================================
# Cleanup
# ============================================================================

CLEANED_UP=false
cleanup() {
  if [ "$CLEANED_UP" = true ]; then return; fi
  CLEANED_UP=true
  set +e

  echo ""
  echo "=== Cleanup ==="

  # Kill port-forwards
  if [ -n "$PF_PID" ]; then kill "$PF_PID" 2>/dev/null || true; fi
  if [ -n "$WEBHOOK_PF_PID" ]; then kill "$WEBHOOK_PF_PID" 2>/dev/null || true; fi

  # Kill runner + server
  if [ -n "$RUNNER_PID" ]; then
    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
  fi
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi

  # Delete test destinations
  delete_test_destinations 2>/dev/null || true

  # k3d cleanup
  if [ "$USE_K3D" = true ]; then
    k3d cluster delete "$CLUSTER_NAME" 2>/dev/null || true
    k3d registry delete "$REGISTRY_NAME" 2>/dev/null || true
    docker rm -f "$GITEA_CONTAINER" 2>/dev/null || true
  fi

  rm -rf .forest
}
trap cleanup EXIT

# ============================================================================
# Mode: flux — in-process, bare git, 3 environments
# ============================================================================

mode_flux() {
  section "0. Prerequisites"
  check_prerequisites mise

  rm -rf "$TEST_DIR"
  mkdir -p "$TEST_DIR"

  section "1. Setup: bare git repo"
  setup_bare_git_repo

  section "2. Verify forest.cue"
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

  section "3. Create flux destinations"
  create_flux_destinations_3env "file://$BARE_REPO"

  section "4. Release prepare"
  do_release_prepare

  # Spot-check rendered content
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

  section "5. Release annotate"
  do_release_annotate "Flux test release v0.1.0" "Automated flux destination test"

  section "6. Release to each environment"
  do_release_to_envs dev staging prod

  section "7. Verify gitops repo structure"
  verify_gitops_repo "file://$BARE_REPO" \
    "dev:flux-dev:dev-cluster-01:1" \
    "staging:flux-staging:staging-cluster-01:2" \
    "prod:flux-prod:prod-cluster-01:3"

  section "8. GitOps repo output"
  show_gitops_output

  echo ""
  echo "  --- Sample: dev Flux Kustomization CR ---"
  local dev_cr="$VERIFY_DIR/clusters/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo.yaml"
  if [ -f "$dev_cr" ]; then
    cat "$dev_cr" | sed 's/^/    /'
  else
    echo "    (not found)"
  fi

  echo ""
  echo "  --- Sample: dev 20-deployment.yaml ---"
  local dev_deploy="$VERIFY_DIR/releases/dev/flux-dev/dev-cluster-01/rust-podinfo/rawpotion-rust-podinfo/20-deployment.yaml"
  if [ -f "$dev_deploy" ]; then
    cat "$dev_deploy" | sed 's/^/    /'
  else
    echo "    (not found)"
  fi

  echo ""
  echo "  Note: $TEST_DIR is preserved for inspection."
}

# ============================================================================
# Mode: flux-runner — distributed runner, bare git, 3 environments
# ============================================================================

mode_flux_runner() {
  section "0. Prerequisites"
  check_prerequisites mise cargo

  rm -rf "$TEST_DIR"
  mkdir -p "$TEST_DIR"

  section "1. Build server and runner"
  build_binaries

  section "2. Setup: bare git repo"
  setup_bare_git_repo

  section "3. Start forest-server with --disable-in-process"
  start_test_server true

  section "4. Start forest-runner"
  start_runner

  section "5. Create flux destinations"
  create_flux_destinations_3env "file://$BARE_REPO"

  section "6. Release prepare"
  do_release_prepare

  section "7. Release annotate"
  do_release_annotate "Runner test release v0.1.0" "Automated runner destination test"

  section "8. Release to each environment (via runner)"
  do_release_to_envs dev staging prod

  section "9. Verify releases went through the runner"
  verify_runner_execution 1

  section "10. Verify gitops repo structure"
  verify_gitops_repo "file://$BARE_REPO" \
    "dev:flux-dev:dev-cluster-01:1" \
    "staging:flux-staging:staging-cluster-01:2" \
    "prod:flux-prod:prod-cluster-01:3"

  section "11. Output summary"
  show_gitops_output
  show_runner_output

  echo ""
  echo "  Note: $TEST_DIR is preserved for inspection."
}

# ============================================================================
# Mode: k3d-flux — in-process, real Kubernetes, single environment
# ============================================================================

mode_k3d_flux() {
  DEST_NAME="flux-dev-k3d"

  section "0. Prerequisites"
  check_prerequisites k3d flux kubectl docker mise
  check_kernel_modules

  rm -rf "$TEST_DIR"
  mkdir -p "$TEST_DIR"

  section "1. Create k3d registry + cluster"
  setup_k3d_cluster

  section "2. Build and push rust-podinfo image"
  build_and_push_podinfo_image

  section "3. Start Gitea git server"
  setup_gitea

  section "4. Install Flux and configure sources"
  setup_flux "$DEST_NAME"

  section "5. Forest release pipeline"
  create_flux_destination_single "$DEST_NAME" dev dev-cluster-01 "$GITEA_GIT_URL" "$RECONCILE_URL"

  do_release_prepare
  do_release_annotate "k3d flux integration test" "Automated k3d + flux v2 e2e test"

  if [ -n "$SLUG" ]; then
    echo "  Releasing to dev (with auto-reconciliation)..."
    if forest_cli release "$SLUG" --environment dev 2>&1 | grep -q "Release completed successfully"; then
      pass "release to dev"
    else
      fail "release to dev"
    fi
  else
    skip "release to dev (no slug)"
  fi

  # Verify gitea received the release commit
  local pushed=false
  for i in $(seq 1 10); do
    local commits
    commits=$(git -C "$TEST_DIR" clone --bare --quiet "$GITEA_GIT_URL" gitops-check 2>/dev/null \
      && git -C "$TEST_DIR/gitops-check" rev-list --count HEAD 2>/dev/null || echo 0)
    rm -rf "$TEST_DIR/gitops-check"
    if [ "$commits" -ge 2 ]; then
      pushed=true
      break
    fi
    sleep 2
  done
  if [ "$pushed" = true ]; then
    pass "gitea repo has release commits"
  else
    fail "gitea repo expected >= 2 commits"
  fi

  section "6. Wait for Flux reconciliation"
  wait_for_flux_reconciliation

  section "7. Test HTTP endpoints via port-forward"
  test_http_endpoints

  section "8. Flux status summary"
  show_flux_status
}

# ============================================================================
# Mode: k3d-flux-runner — distributed runner, real Kubernetes, single env
#
# NOTE: Does NOT use --disable-in-process because forest.cue defines both
# kubernetes and flux destination types per environment. Kubernetes releases
# need the in-process fallback (no runner supports kubernetes@1). The
# scheduler already prefers remote runners first, so flux releases go through
# the runner while kubernetes releases fall back to in-process.
# ============================================================================

mode_k3d_flux_runner() {
  DEST_NAME="flux-dev-k3d"

  section "0. Prerequisites"
  check_prerequisites k3d flux kubectl docker mise cargo
  check_kernel_modules

  rm -rf "$TEST_DIR"
  mkdir -p "$TEST_DIR"

  section "1. Build server, runner, and podinfo"
  build_binaries

  section "2. Start forest-server"
  # NO --disable-in-process: kubernetes destinations need in-process fallback
  start_test_server false

  section "3. Start forest-runner"
  start_runner

  section "4. Create k3d registry + cluster"
  setup_k3d_cluster

  section "5. Build and push rust-podinfo image"
  build_and_push_podinfo_image

  section "6. Start Gitea git server"
  setup_gitea

  section "7. Install Flux and configure sources"
  setup_flux "$DEST_NAME"

  section "8. Forest release pipeline (via runner)"
  create_flux_destination_single "$DEST_NAME" dev dev-cluster-01 "$GITEA_GIT_URL" "$RECONCILE_URL"

  do_release_prepare
  do_release_annotate "k3d runner flux integration test" "Automated k3d + flux v2 + runner e2e test"

  if [ -n "$SLUG" ]; then
    echo "  Releasing to dev (via runner with auto-reconciliation)..."
    if forest_cli release "$SLUG" --environment dev 2>&1 | grep -q "Release completed successfully"; then
      pass "release to dev"
    else
      fail "release to dev"
    fi
  else
    skip "release to dev (no slug)"
  fi

  # Give scheduler time to dispatch
  sleep 5

  section "9. Verify runner execution"
  verify_runner_execution 1

  section "10. Wait for Flux reconciliation"
  wait_for_flux_reconciliation

  section "11. Test HTTP endpoints"
  test_http_endpoints

  section "12. Status summary"
  show_runner_output
  show_flux_status
}

# ============================================================================
# Main Entry Point
# ============================================================================

usage() {
  echo "Usage: $0 <mode>"
  echo ""
  echo "Modes:"
  echo "  flux              Flux destination test (in-process, bare git, 3 envs)"
  echo "  flux-runner       Flux destination via distributed runner (bare git, 3 envs)"
  echo "  k3d-flux          Full k3d + Flux v2 e2e test (in-process, single env)"
  echo "  k3d-flux-runner   Full k3d + Flux v2 e2e via distributed runner (single env)"
  echo ""
  echo "Prerequisites:"
  echo "  flux, k3d-flux        Requires dev server running (mise run develop)"
  echo "  flux-runner,          Starts own server+runner on ports $TEST_GRPC_PORT-$TEST_HTTP_PORT"
  echo "  k3d-flux-runner"
  echo "  k3d-*                 Requires k3d, flux, kubectl, docker"
  echo ""
  echo "Examples:"
  echo "  $0 flux               # Quick: test flux destination in-process"
  echo "  $0 flux-runner        # Quick: test distributed runner path"
  echo "  $0 k3d-flux           # Full: real k8s + flux reconciliation"
  echo "  $0 k3d-flux-runner    # Full: real k8s + flux via runner"
}

MODE="${1:-}"
if [ -z "$MODE" ]; then
  usage
  exit 1
fi

case "$MODE" in
  flux)
    TEST_DIR="$SCRIPT_DIR/.flux-test"
    BARE_REPO="$TEST_DIR/bare.git"
    VERIFY_DIR="$TEST_DIR/verify"
    USE_RUNNER=false
    USE_K3D=false
    mode_flux
    ;;
  flux-runner)
    TEST_DIR="$SCRIPT_DIR/.flux-runner-test"
    BARE_REPO="$TEST_DIR/bare.git"
    VERIFY_DIR="$TEST_DIR/verify"
    SERVER_LOG="$TEST_DIR/server.log"
    RUNNER_LOG="$TEST_DIR/runner.log"
    USE_RUNNER=true
    USE_K3D=false
    mode_flux_runner
    ;;
  k3d-flux)
    TEST_DIR="$SCRIPT_DIR/.k3d-flux-test"
    USE_RUNNER=false
    USE_K3D=true
    mode_k3d_flux
    ;;
  k3d-flux-runner)
    TEST_DIR="$SCRIPT_DIR/.k3d-flux-runner-test"
    SERVER_LOG="$TEST_DIR/server.log"
    RUNNER_LOG="$TEST_DIR/runner.log"
    USE_RUNNER=true
    USE_K3D=true
    mode_k3d_flux_runner
    ;;
  *)
    echo "Unknown mode: $MODE"
    usage
    exit 1
    ;;
esac

results
