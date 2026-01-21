FROM rust:1.92-slim AS builder

WORKDIR /app

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    ca-certificates \
    curl \
    gcc g++ clang cmake ninja-build \
    libsqlite3-dev libzstd-dev liblz4-dev libssl-dev zlib1g-dev libssl-dev \
    git ssh \
    && rm -rf /var/lib/apt/lists/*


# Setup git ssh, and load the public key for known hosts
RUN \
  git config --global url."ssh://git@github.com".insteadOf https://github.com && \
  git config --global url."ssh://git@git.kjuulh.io".insteadOf https://git.kjuulh.io && \
  mkdir -p -m 0600 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts && \
  mkdir -p -m 0600 ~/.ssh && ssh-keyscan git.kjuulh.io >> ~/.ssh/known_hosts 
    
COPY . .

RUN --mount=type=ssh cargo build --release --bin forest-server

FROM debian:13-slim AS production

ENV TERRAFORM_EXE=tofu

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl apt-transport-https gnupg \
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
    && apt-get install -y tofu \
    && tofu --help

WORKDIR /app

COPY --from=builder /app/target/release/forest-server /app/forest-server

RUN chmod +x /app/forest-server

RUN groupadd -r appuser && useradd -r -g appuser appuser
RUN chown -R appuser:appuser /app
RUN chown appuser:appuser /usr/bin/tofu
USER appuser

RUN /app/forest-server --help

CMD ["/app/forest-server"]
