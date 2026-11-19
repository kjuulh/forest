FROM rust:1.100-trixie AS chef

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git ssh ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN git config --global url."ssh://git@github.com".insteadOf https://github.com && \
    mkdir -p -m 0600 ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts

RUN cargo install cargo-chef --locked --version 0.1.78

WORKDIR /mnt/src

ENV SQLX_OFFLINE=true

FROM chef AS planner

COPY . .

RUN --mount=type=ssh cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /mnt/src/recipe.json recipe.json

RUN --mount=type=ssh cargo chef cook --release -p forage-server --recipe-path recipe.json

COPY . .

RUN --mount=type=ssh cargo build --release -p forage-server

FROM gcr.io/distroless/cc-debian13:latest

COPY --from=builder /mnt/src/target/release/forage-server /usr/local/bin/forage-server
COPY --from=builder /mnt/src/templates /templates
COPY --from=builder /mnt/src/static /static

ARG GIT_SHA=""
ARG BUILD_TIME=""
ENV FOREST_GIT_SHA=$GIT_SHA
ENV FOREST_BUILD_TIME=$BUILD_TIME

WORKDIR /
ENV FORAGE_TEMPLATES_PATH=/templates

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/forage-server"]
