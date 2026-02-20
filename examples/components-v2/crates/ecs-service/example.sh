#!/usr/bin/env bash
set -euo pipefail

SPEC='{
  "cpu": "256",
  "memory": "512",
  "name": "my-api",
  "image": "my-api:latest",
  "replicas": 2,
  "environment": "dev",
  "ports": [{"name": "http", "port": 8080, "protocol": "tcp", "external": true}],
  "health_check": {"path": "/health", "interval": 30, "timeout": 5, "retries": 3}
}'

PAYLOAD=$(jq -nc --argjson spec "$SPEC" '{"spec": $spec}')

echo "=== commands/prepare ==="
cargo run -p ecs-service -- commands/prepare "$PAYLOAD"

echo ""
echo "=== commands/status ==="
cargo run -p ecs-service -- commands/status "$PAYLOAD"

echo ""
echo "=== hooks/forest/deployment/release ==="
cargo run -p ecs-service -- hooks/forest/deployment/release \
  "$(jq -nc --argjson spec "$SPEC" '{"spec": $spec, "input": {"release_id": "rel-42"}}')"

echo ""
echo "=== commands/status (via stdin) ==="
echo "$PAYLOAD" | cargo run -p ecs-service -- commands/status
