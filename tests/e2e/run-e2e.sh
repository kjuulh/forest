#!/usr/bin/env bash
set -euo pipefail

# Run the full E2E test suite in Docker.
# Usage: ./tests/e2e/run-e2e.sh

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$PROJECT_ROOT"

echo "=== Building forest binaries ==="
SQLX_OFFLINE=true cargo build -p forest -p forest-server

echo ""
echo "=== Copying binaries for Docker ==="
mkdir -p tests/e2e/bin
cp target/debug/forest tests/e2e/bin/forest
cp target/debug/forest-server tests/e2e/bin/forest-server

echo ""
echo "=== Starting E2E test environment ==="
cd tests/e2e

docker compose down -v 2>/dev/null || true
docker compose build

# Start infra + server in background
docker compose up -d postgres nats minio minio-init forest-server

# Wait for server to be healthy
echo "Waiting for forest-server to be healthy..."
for i in $(seq 1 60); do
    if docker compose exec -T forest-server curl -sf http://localhost:4042/v2/ > /dev/null 2>&1; then
        echo "forest-server is ready."
        break
    fi
    if [ "$i" -eq 60 ]; then
        echo "Timed out waiting for forest-server"
        docker compose logs forest-server
        docker compose down -v
        rm -rf bin/
        exit 1
    fi
    sleep 1
done

# Run the e2e test container
echo ""
echo "=== Running E2E tests ==="
docker compose run --rm e2e
EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "E2E tests passed."
else
    echo "E2E tests FAILED (exit code $EXIT_CODE)."
    echo ""
    echo "Server logs:"
    docker compose logs forest-server 2>/dev/null | tail -30 || true
fi

docker compose down -v
rm -rf bin/

exit $EXIT_CODE
