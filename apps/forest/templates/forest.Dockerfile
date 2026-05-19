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

RUN --mount=type=ssh cargo build --release --bin forest

FROM debian:13-slim AS production

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/forest /app/forest

RUN chmod +x /app/forest

RUN groupadd -r appuser && useradd -r -g appuser appuser
RUN chown -R appuser:appuser /app
USER appuser

RUN /app/forest --help

CMD ["/app/forest"]
