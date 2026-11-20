FROM rust:1.100.0-trixie AS builder

RUN apt-get update && \
    apt-get install -y clang mold && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /mnt/src

RUN mkdir -p /usr/local/cargo && \
    printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' \
    > /usr/local/cargo/config.toml

ENV SQLX_OFFLINE=true

COPY Cargo.toml Cargo.lock ./
COPY crates/forage-server/Cargo.toml crates/forage-server/Cargo.toml
COPY crates/forage-core/Cargo.toml crates/forage-core/Cargo.toml
COPY crates/forage-db/Cargo.toml crates/forage-db/Cargo.toml

RUN mkdir -p crates/forage-server/src && echo 'fn main() {}' > crates/forage-server/src/main.rs && \
    mkdir -p crates/forage-core/src && echo '' > crates/forage-core/src/lib.rs && \
    mkdir -p crates/forage-db/src && echo '' > crates/forage-db/src/lib.rs

RUN cargo build --release -p forage-server 2>/dev/null || true

COPY crates/ crates/
COPY templates/ templates/
COPY static/ static/
COPY .sqlx/ .sqlx/

RUN find crates -name "*.rs" -exec touch {} +

RUN cargo build --release -p forage-server

RUN ./target/release/forage-server --help || true

FROM gcr.io/distroless/cc-debian13:latest

COPY --from=builder /mnt/src/target/release/forage-server /usr/local/bin/forage-server
COPY --from=builder /mnt/src/static /static

WORKDIR /

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/forage-server"]
