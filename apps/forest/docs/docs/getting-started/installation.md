# Installation

## From Source (Cargo)

Forest is written in Rust. Install it with Cargo:

```bash
cargo install --path crates/forest
```

Or if you have the repository cloned and use [mise](https://mise.jdx.dev/):

```bash
mise run install
```

This builds and installs the `forest` binary to your Cargo bin directory.

## Verify Installation

```bash
forest --version
forest --help
```

## Requirements

- **Rust 1.93+** — Forest uses recent Rust features
- **CUE** — Required for evaluating component specs (`cue` CLI)
- **Git** — For release context (commit SHA, branch, etc.)

### Optional

- **Docker** — For building Docker-based components
- **kubectl** — For Kubernetes destinations
- **Terraform** — For Terraform destinations

## Server Setup

Forest requires a running Forest server for release management, the component registry, and organisation features. For local development:

```bash
# Start PostgreSQL and NATS via Docker Compose
mise run local:up

# Run database migrations
mise run db:migrate

# Start the server
mise run dev
```

The server starts on `http://localhost:4040` by default.

## Configuration

Forest looks for server configuration in this order:

1. `--server` CLI flag
2. `FOREST_SERVER` environment variable
3. Stored credentials from `forest auth login`
