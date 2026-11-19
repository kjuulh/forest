FROM rust:1.100-trixie AS chef

WORKDIR /app

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    ca-certificates \
    curl \
    git ssh \
    && rm -rf /var/lib/apt/lists/*

RUN git config --global url."ssh://git@github.com".insteadOf https://github.com && \
    mkdir -p -m 0600 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts

ENV SQLX_OFFLINE=true

RUN cargo install cargo-chef --locked --version 0.1.78

FROM chef AS planner

COPY . .

RUN --mount=type=ssh cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json

RUN --mount=type=ssh cargo chef cook --release \
    --bin forest-server --bin forest-event-migration --recipe-path recipe.json

COPY . .

RUN --mount=type=ssh cargo build --release --bin forest-server --bin forest-event-migration

FROM debian:13-slim AS production

ENV TERRAFORM_EXE=tofu

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

ARG GIT_SHA=""
ARG BUILD_TIME=""
ENV FOREST_GIT_SHA=$GIT_SHA
ENV FOREST_BUILD_TIME=$BUILD_TIME

ENTRYPOINT ["forest-server"]
