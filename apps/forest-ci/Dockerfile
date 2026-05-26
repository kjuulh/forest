# forest-ci — single image consumed by the woodpecker plugin and (later)
# a GitHub Action. Bakes the forest CLI, cue, and deno so consuming repos
# don't need 25 lines of curl bootstrap in every deploy step.
#
# Pin versions via build args so the image can be rebuilt without
# editing the Dockerfile. The tag pattern in .woodpecker/forest-ci-image.yaml
# is `:rawpotion` (rolling) + `:rawpotion-<sha>` (immutable).

FROM debian:bookworm-slim

ARG FOREST_VERSION=v0.1.14
ARG CUE_VERSION=v0.15.4
ARG DENO_VERSION=v2.7.14
ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates unzip jq \
    && rm -rf /var/lib/apt/lists/*

RUN case "$TARGETARCH" in \
      amd64) FOREST_TARGET=x86_64-unknown-linux-gnu ; CUE_ARCH=amd64 ; DENO_TARGET=x86_64-unknown-linux-gnu ;; \
      arm64) FOREST_TARGET=aarch64-unknown-linux-gnu ; CUE_ARCH=arm64 ; DENO_TARGET=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://src.rawpotion.io/rawpotion/forest/releases/download/${FOREST_VERSION}/forest-${FOREST_VERSION}-${FOREST_TARGET}.tar.gz" \
      | tar xz -C /usr/local/bin forest && \
    curl -fsSL "https://github.com/cue-lang/cue/releases/download/${CUE_VERSION}/cue_${CUE_VERSION}_linux_${CUE_ARCH}.tar.gz" \
      | tar xz -C /usr/local/bin cue && \
    curl -fsSL "https://github.com/denoland/deno/releases/download/${DENO_VERSION}/deno-${DENO_TARGET}.zip" -o /tmp/deno.zip && \
    unzip -o -j /tmp/deno.zip -d /usr/local/bin && rm /tmp/deno.zip && \
    chmod +x /usr/local/bin/forest /usr/local/bin/cue /usr/local/bin/deno && \
    forest --version && cue version && deno --version | head -1

COPY entrypoint.sh /usr/local/bin/forest-ci
RUN chmod +x /usr/local/bin/forest-ci

# Default: pick up env, run the action. Woodpecker's plugin convention
# is no CMD/ENTRYPOINT override needed — woodpecker invokes
# /usr/local/bin/forest-ci as the container's command via the image's
# ENTRYPOINT.
ENTRYPOINT ["/usr/local/bin/forest-ci"]
