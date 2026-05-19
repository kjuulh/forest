FROM rust:1.93-trixie AS builder

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

RUN --mount=type=ssh cargo build --release --bin forest-server

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

ENTRYPOINT ["forest-server"]
