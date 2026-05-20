# Forest

> Codify your development workflows — CI, deployments, component sharing — as
> [CUE](https://cuelang.org/) manifests, then share them across your
> organisation.

This repository hosts the Forest CLI **and** its companion managed platform:

| Path | What |
|---|---|
| [`apps/forest/`](apps/forest/) | The `forest` CLI, gRPC server (forest-server), Rust SDK + codegen, and the CUE component library. End users normally only need the CLI. |
| [`apps/forage/`](apps/forage/) | Forage — the BFF / web app that puts Forest's APIs behind a browser-friendly UI. Hosted at [forage.understory.sh](https://forage.understory.sh). |

## Install the CLI

The `forest` repository is private. The installer wraps
[`gh release download`](https://cli.github.com/manual/gh_release_download) so it
reuses the GitHub CLI's existing auth instead of asking you to manage a
separate token.

```bash
# 1. One-time prereq (skip if `gh auth status` already shows you signed in)
gh auth login

# 2. Install
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash
```

Pin a version, install to a non-system prefix, or run offline from a downloaded
script — any of these work:

```bash
# Specific tag
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash -s -- v0.2.0

# Install under ~/.local/bin instead of /usr/local/bin (no sudo)
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | PREFIX=$HOME/.local bash

# Download then run (e.g. for an audit before piping to bash)
gh release download --repo understory-io/forest --pattern install.sh
bash install.sh                       # latest
bash install.sh v0.2.0                # pinned
```

The script detects your platform (`aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`), verifies the SHA-256
of the release tarball, and installs `forest` into `$PREFIX/bin` (default
`/usr/local/bin`).

Verify the install:

```bash
forest --version
```

### Building from source

```bash
gh repo clone understory-io/forest
cd forest
cargo install --path apps/forest/crates/forest --locked
```

Requires Rust 1.93 (pinned via [mise](https://mise.jdx.dev/)).

## Releases

Releases are managed by
[`release-please`](https://github.com/googleapis/release-please): merge a
conventional commit with a `(forest)` scope to `main`
(`feat(forest): …`, `fix(forest): …`), and release-please opens a release PR
that bumps the version and updates the changelog. Merging that PR cuts a tag
and triggers `.github/workflows/release.yml`, which builds binaries for all
three supported platforms, attaches them to the GitHub release, and mirrors
the installer to the public `homebrew-tap` repo.

The flow lives in two files:

- [`.github/workflows/release-please.yml`](.github/workflows/release-please.yml) — opens / updates release PRs
- [`.github/workflows/release.yml`](.github/workflows/release.yml) — builds binaries + publishes installer

## Repository layout

```
.
├── apps/
│   ├── forest/        # CLI + server + SDK + components
│   └── forage/        # BFF / web app
├── scripts/
│   └── install.sh     # one-stop forest CLI installer (mirrored to homebrew-tap)
└── .github/workflows/ # CI, release-please, release
```

Each app is its own Cargo workspace with its own `mise.toml` for local tasks
(see `apps/forest/mise.toml` and `apps/forage/mise.toml`).

## Development

Bring up both apps' local stacks (Postgres, NATS, MinIO, etc.):

```bash
mise install                    # installs Rust 1.93 + tooling
mise run local:up               # both stacks
mise run forest:local:up        # forest only
mise run forage:local:up        # forage only
```

See each app's own README for details: [forest](apps/forest/README.md).
