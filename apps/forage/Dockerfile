FROM rust:1.93-trixie AS builder

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        clang mold ca-certificates curl nodejs npm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /mnt/src

# sccache — see apps/forest/Dockerfile for the rationale. Mirror that
# block here so the forage build participates in the same memcached
# cache when SCCACHE_MEMCACHED_ENDPOINT is provided by CI.
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

# Build frontend assets inside the image instead of relying on checked-in
# generated files. The package locks make Woodpecker's Docker image match the
# source commit exactly.
RUN npm ci && \
    (cd frontend && npm ci && npm run build) && \
    npx tailwindcss -i static/css/input.css -o static/css/style.css --minify

# Probe the sccache backend before building. If SCCACHE_MEMCACHED_ENDPOINT
# is empty, remove it so sccache uses its default disk cache. If it is set but
# unreachable, drop RUSTC_WRAPPER so cargo builds normally instead of failing.
# See apps/forest/Dockerfile for the rationale.
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
    cargo build --release --locked -p forage-server; \
    if [ -n "${RUSTC_WRAPPER:-}" ]; then sccache --show-stats; fi

FROM gcr.io/distroless/cc-debian13:latest

COPY --from=builder /mnt/src/target/release/forage-server /usr/local/bin/forage-server
COPY --from=builder /mnt/src/templates /templates
COPY --from=builder /mnt/src/static /static

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

WORKDIR /
ENV FORAGE_TEMPLATES_PATH=/templates

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/forage-server"]
