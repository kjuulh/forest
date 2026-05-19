FROM rust:1.95 AS builder

WORKDIR /app
COPY . .

RUN cargo build --release --bin ecs-service

FROM debian:13-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/ecs-service /usr/local/bin/ecs-service

ENTRYPOINT ["ecs-service"]
