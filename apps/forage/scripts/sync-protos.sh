#!/usr/bin/env bash
set -euo pipefail

FOREST_PROTO="/home/kjuulh/git/src.rawpotion.io/rawpotion/forest/interface/proto/forest/v1"
FORAGE_PROTO="/home/kjuulh/git/git.kjuulh.io/forage/client/interface/proto/forest/v1"

echo "Syncing protos from forest -> forage..."

for proto in "$FOREST_PROTO"/*.proto; do
    name=$(basename "$proto")
    cp "$proto" "$FORAGE_PROTO/$name"
    echo "  copied $name"
done

echo "Running buf generate..."
cd /home/kjuulh/git/git.kjuulh.io/forage/client
buf generate

echo "Done."
