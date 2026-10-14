FROM rust:1.98-trixie AS chef

WORKDIR /app

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    ca-certificates \
    curl \
    clang mold git ssh \
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

ENV SQLX_OFFLINE=true

# The installer runs before the builder's cache reachability probe. A TCP-open
# memcached endpoint can still hang sccache requests for ten minutes; compile
# this small build tool locally so a cache outage cannot stall every image.
RUN RUSTC_WRAPPER= SCCACHE_MEMCACHED_ENDPOINT= cargo install cargo-chef --locked --version 0.1.78

# The recipe is a digest of every manifest in the workspace and nothing else, so
# this stage re-runs on any commit but its *output* only changes when a
# Cargo.toml or Cargo.lock does.
FROM chef AS planner

COPY . .

RUN --mount=type=ssh cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Dependencies compile against the recipe alone. This layer is keyed on
# recipe.json, so it survives every commit that does not touch a manifest --
# which is most of them. `.sqlx/` is not needed here: cook builds the
# dependency graph against a skeleton workspace, and no sqlx macro runs until
# the real sources arrive below.
# sccache can hang during dependency cook even when memcached accepts TCP
# connections. Compile this layer locally; the final app build still uses its
# cache reachability probe below.
COPY --from=planner /app/recipe.json recipe.json

RUN --mount=type=ssh RUSTC_WRAPPER= SCCACHE_MEMCACHED_ENDPOINT= cargo chef cook --release \
    --bin forest-server --bin forest-event-migration --recipe-path recipe.json

COPY . .

# Probe the sccache backend before building. If SCCACHE_MEMCACHED_ENDPOINT
# is empty, remove it so sccache uses its default disk cache. If it is set but
# unreachable, drop RUSTC_WRAPPER so cargo builds normally instead of failing.
# The SSH mount remains available for private Git dependencies while they exist.
RUN --mount=type=ssh set -e; \
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
    cargo build --release --locked --bin forest-server --bin forest-event-migration; \
    if [ -n "${RUSTC_WRAPPER:-}" ]; then sccache --show-stats; fi

FROM debian:13-slim AS production

ENV TERRAFORM_EXE=tofu

# The OpenTofu every forest/terraform@1 plan and apply runs with. Pinned, not
# "whatever apt has today" (forest#283): state written by a newer tofu can be
# unreadable by an older one, so an unpinned install made a routine image
# rebuild a silent state-format upgrade — and rolling the image back after it
# could strand every terraform destination. Components also lean on
# version-gated features (`import` blocks, `removed` blocks, `for_each` on
# import), which want a known floor.
#
# To bump: pick a release from https://github.com/opentofu/opentofu/releases,
# check its upgrade notes for state-format changes, change the number here, and
# say in the PR that it is a tofu bump. Rolling back past a bump is only safe if
# no apply has run on the newer version since.
ARG TOFU_VERSION=1.12.6

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
    && apt-get install -y "tofu=${TOFU_VERSION}" \
    && tofu version | grep -q "v${TOFU_VERSION}"

WORKDIR /app

COPY --from=builder /app/target/release/forest-server /usr/local/bin/forest-server
COPY --from=builder /app/target/release/forest-event-migration /usr/local/bin/forest-event-migration

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
