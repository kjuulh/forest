# Forest

Codify your development workflows — CI, deployments, component sharing — as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

```bash
gh auth login   # one-time
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash
```

Check it works:

```bash
forest --version
```

### Other ways

```bash
# Pin a specific version
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash -s -- v0.2.0

# Install under ~/.local/bin instead of /usr/local/bin (no sudo)
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | PREFIX=$HOME/.local bash
```

## What's here

- [`apps/forest/`](apps/forest/) — the `forest` CLI and supporting libraries
- [`apps/forage/`](apps/forage/) — the managed web UI ([forage.understory.sh](https://forage.understory.sh))
