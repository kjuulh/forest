FROM rust:1.98-trixie AS builder

WORKDIR /app

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    ca-certificates \
    curl \
    clang mold \
    git ssh \
    && rm -rf /var/lib/apt/lists/*

RUN git config --global url."ssh://git@github.com".insteadOf https://github.com && \
    mkdir -p -m 0600 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts

RUN mkdir -p /usr/local/cargo && \
    printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' \
    > /usr/local/cargo/config.toml

ENV SQLX_OFFLINE=true

COPY . .

RUN --mount=type=ssh cargo build --release --bin forest-server --bin forest-event-migration

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
