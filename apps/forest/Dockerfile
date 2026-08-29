FROM rust:1.93-trixie AS builder

WORKDIR /app

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    ca-certificates \
    curl \
    clang mold \
    && rm -rf /var/lib/apt/lists/*

# sccache amortizes rustc compile across CI runs. When
# SCCACHE_MEMCACHED_ENDPOINT is set (passed from .woodpecker/ci.yaml)
# rustc invocations are cached in the homelab memcached server; when
# empty (e.g. local docker build) sccache falls back to its disk cache,
# which is per-image and effectively a no-op but harmless.
#
# The memcached host lives on the internal VLAN and isn't always
# reachable (PR builds on foreign runners, the cache host being down,
# etc). sccache would otherwise fail the whole build when it can't
# reach its backend, so the build step below probes the endpoint first
# and, if unreachable, drops RUSTC_WRAPPER so cargo compiles normally
# without caching instead of erroring out.
ARG SCCACHE_VERSION=0.9.1
ARG SCCACHE_MEMCACHED_ENDPOINT=
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) SCCACHE_TARGET=x86_64-unknown-linux-musl ;; \
      arm64) SCCACHE_TARGET=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-${SCCACHE_TARGET}.tar.gz" \
      | tar xz -C /tmp && \
    install -m 0755 "/tmp/sccache-v${SCCACHE_VERSION}-${SCCACHE_TARGET}/sccache" /usr/local/bin/sccache && \
    sccache --version
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_MEMCACHED_ENDPOINT=$SCCACHE_MEMCACHED_ENDPOINT

RUN mkdir -p /usr/local/cargo && \
    printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' \
    > /usr/local/cargo/config.toml

ENV SQLX_OFFLINE=true

COPY . .

# Probe the sccache backend before building. If SCCACHE_MEMCACHED_ENDPOINT
# is empty, remove it so sccache uses its default disk cache. If it is set but
# unreachable, drop RUSTC_WRAPPER so cargo builds normally instead of failing.
RUN set -e; \
    if [ -n "$SCCACHE_MEMCACHED_ENDPOINT" ]; then \
      hostport="${SCCACHE_MEMCACHED_ENDPOINT#tcp://}"; \
      host="${hostport%%:*}"; port="${hostport##*:}"; \
      if timeout 5 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null; then \
        echo "sccache: memcached reachable at $host:$port — caching enabled"; \
      else \
        echo "sccache: memcached unreachable at $host:$port — building without rustc cache"; \
        unset RUSTC_WRAPPER SCCACHE_MEMCACHED_ENDPOINT; \
      fi; \
    else \
      unset SCCACHE_MEMCACHED_ENDPOINT; \
    fi; \
    cargo build --release --locked --bin forest-server; \
    if [ -n "${RUSTC_WRAPPER:-}" ]; then sccache --show-stats; fi

FROM debian:13-slim AS production

ENV TERRAFORM_EXE=tofu

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl apt-transport-https gnupg git \
    && rm -rf /var/lib/apt/lists/* \
    && install -m 0755 -d /etc/apt/keyrings \
    && curl -fsSL https://get.opentofu.org/opentofu.gpg | tee /etc/apt/keyrings/opentofu.gpg >/dev/null \
    && curl -fsSL https://packages.opentofu.org/opentofu/tofu/gpgkey | gpg --no-tty --batch --dearmor -o /etc/apt/keyrings/opentofu-repo.gpg >/dev/null \
    && chmod a+r /etc/apt/keyrings/opentofu.gpg /etc/apt/keyrings/opentofu-repo.gpg \
    && echo \
  "deb [signed-by=/etc/apt/keyrings/opentofu.gpg,/etc/apt/keyrings/opentofu-repo.gpg] https://packages.opentofu.org/opentofu/tofu/any/ any main \n\
deb-src [signed-by=/etc/apt/keyrings/opentofu.gpg,/etc/apt/keyrings/opentofu-repo.gpg] https://packages.opentofu.org/opentofu/tofu/any/ any main" | \
  tee /etc/apt/sources.list.d/opentofu.list > /dev/null \
    && chmod a+r /etc/apt/sources.list.d/opentofu.list \
    && apt-get update \
    && apt-get install -y tofu

WORKDIR /app

COPY --from=builder /app/target/release/forest-server /usr/local/bin/forest-server

RUN groupadd -r appuser && useradd -r -g appuser appuser \
    && chown -R appuser:appuser /app \
    && chown appuser:appuser /usr/bin/tofu
USER appuser

# Build provenance, baked into the final stage only (DATA-505 follow-up).
#
# Deliberately after the compile: an ARG/ENV before `cargo build` changes the
# build environment on every commit and invalidates the cached compile. Placing
# it here means the image still self-describes (`docker inspect` shows both
# values, and the server reads them at startup) while the expensive builder
# layers stay cacheable.
ARG GIT_SHA=""
ARG BUILD_TIME=""
ENV FOREST_GIT_SHA=$GIT_SHA
ENV FOREST_BUILD_TIME=$BUILD_TIME

ENTRYPOINT ["forest-server"]
