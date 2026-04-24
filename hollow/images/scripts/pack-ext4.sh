#!/bin/bash
# Pack a Docker image into a raw ext4 filesystem image for Firecracker.
#
# Usage: pack-ext4.sh <docker-image-tag> <output.ext4> [size-mb]
#
# Requires: docker, mkfs.ext4, mount (needs root/sudo for mount)

set -euo pipefail

IMAGE="${1:?Usage: pack-ext4.sh <image-tag> <output.ext4> [size-mb]}"
OUTPUT="${2:?Usage: pack-ext4.sh <image-tag> <output.ext4> [size-mb]}"
SIZE_MB="${3:-2048}"

echo "==> Creating ext4 image: ${OUTPUT} (${SIZE_MB} MiB) from ${IMAGE}"

# Create container to export filesystem
CID=$(docker create "${IMAGE}" /bin/true)
trap 'docker rm -f "${CID}" >/dev/null 2>&1 || true' EXIT

# Create and format the image file
dd if=/dev/zero of="${OUTPUT}" bs=1M count="${SIZE_MB}" status=progress
mkfs.ext4 -F "${OUTPUT}"

# Mount, extract, unmount
MOUNT_DIR=$(mktemp -d)
trap 'sudo umount "${MOUNT_DIR}" 2>/dev/null || true; rmdir "${MOUNT_DIR}" 2>/dev/null || true; docker rm -f "${CID}" >/dev/null 2>&1 || true' EXIT

sudo mount -o loop "${OUTPUT}" "${MOUNT_DIR}"
docker export "${CID}" | sudo tar -xf - -C "${MOUNT_DIR}"
sudo umount "${MOUNT_DIR}"
rmdir "${MOUNT_DIR}"

echo "==> Done: ${OUTPUT}"
ls -lh "${OUTPUT}"
