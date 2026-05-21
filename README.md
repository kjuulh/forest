# Forest

Codify your development workflows — CI, deployments, component sharing — as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

```bash
gh auth login   # one-time
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash
```

Check it works (doesn't write any state to disk):

```bash
forest self check
```

### Set up a context during install

Point forest at your server in one step by passing `FOREST_PROFILE` to the
installer. The first context provisioned becomes the active default:

```bash
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh \
  | FOREST_PROFILE='name=understory-prod,server=https://forest.development.understory.sh' bash -s -- v0.1.3
```

The trailing `v0.1.3` pins the forest version; drop it to install the latest.
`CUE_REGISTRY` is derived from the server automatically, so you don't have to
remember to export it. Every command afterwards prints a one-line banner
showing which context it's running against.

If you also want `forest auth login` to default to the browser flow without
extra configuration, add a `web=` key pointing at forage:

```bash
FOREST_PROFILE='name=understory-prod,server=https://forest.development.understory.sh,web=https://forage.development.understory.sh'
```

When `web=` is omitted the CLI falls back to a `forest. → forage.`
convention; set it explicitly if your deployment doesn't match that pattern.

## Logging in

```bash
forest auth login
```

Opens your browser at the active context's forage URL, shows a short
one-time code, and signs you in once you approve. Mirrors `gh auth login`.

- `forest auth login --web` — skip the prompt, go straight to the browser.
- `forest auth login --password` — legacy username/email + password flow
  (still required for scripts that pipe a password from stdin or set
  `FOREST_PASSWORD`).
- `FOREST_WEB_URL=https://forage.example.com` — one-shot override of the
  forage URL the CLI sends the browser to. Useful when testing against a
  staging forage that doesn't match the context's stored `web_url`.

### Other ways

```bash
# Pin a specific version
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | bash -s -- v0.2.0

# Install under ~/.local/bin instead of /usr/local/bin (no sudo)
curl -fsSL https://raw.githubusercontent.com/understory-io/homebrew-tap/main/install-forest.sh | PREFIX=$HOME/.local bash
```

### Shell integration

Add the forest shell integration to your shell rc file so completions and
helper functions are available in every new session:

```bash
# ~/.zshrc (or ~/.bashrc — swap `zsh` for `bash`)
eval "$(forest shell zsh)"
```

This sources the bits forest needs to feel native: tab completion, the
`forest cd` helper, and any per-context env hooks. Run `forest shell --help`
for the full list of supported shells.

### Keeping forest up to date

```bash
forest self check     # is a newer version available?
forest self update    # upgrade to latest
```

A one-line nag also prints at the end of every command when a newer release
exists (cached 24h; suppress with `FOREST_NO_UPDATE_CHECK=1` or `CI=true`).

## What's here

- [`apps/forest/`](apps/forest/) — the `forest` CLI and supporting libraries
- [`apps/forage/`](apps/forage/) — the managed web UI ([forage.understory.sh](https://forage.understory.sh))
