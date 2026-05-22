# Forest

Codify your development workflows — CI, deployments, component sharing — as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

Forest is distributed as a release asset on this repo. Because the repo
is private, the install script uses the GitHub CLI (which you need
authenticated anyway for day-to-day forest access) instead of an
unauthenticated `curl` against a public mirror.

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - | bash
```

The script downloads the platform tarball + checksum, verifies it, and
installs `forest` to `~/.local/bin` — no sudo needed. If `~/.local/bin`
isn't on your `PATH`, the installer also appends an entry to your
`~/.zshrc` / `~/.bashrc` to fix that (idempotent; opt out with
`FOREST_NO_SHELL_INTEGRATION=1`). For a system-wide install, pass
`PREFIX=/usr/local` — that path needs sudo, which the script handles
when a TTY is attached.

Point forest at your server in one step by passing `FOREST_PROFILE` to
the installer — the first context provisioned becomes the active default:

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - \
  | FOREST_PROFILE='name=understory-prod,server=https://forest.development.understory.sh' bash
```

The installer also appends the forest shell integration to your shell rc
file (`~/.zshrc` for zsh, `~/.bashrc` for bash) so completions and
helper functions are available in every new session. Already present →
skipped. To opt out, pass `FOREST_NO_SHELL_INTEGRATION=1`. For other
shells, add this line yourself:

```bash
# ~/.zshrc (or ~/.bashrc — swap `zsh` for `bash`)
command -v forest >/dev/null 2>&1 && eval "$(forest shell zsh)"
```

To pin to a specific version, pass it to both the download (so the script
itself comes from that release) and to the install script (so it grabs
the matching tarball). `bash -s --` forwards positional args when the
script comes in via stdin:

```bash
gh release download v0.1.7 --repo understory-io/forest --pattern install.sh -O - | bash -s -- v0.1.7
```

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
# System-wide install (needs sudo for /usr/local/bin)
gh release download --repo understory-io/forest --pattern install.sh -O - | PREFIX=/usr/local bash

# Arbitrary prefix (still no sudo if you own it)
gh release download --repo understory-io/forest --pattern install.sh -O - | PREFIX=$HOME/opt/forest bash
```

### Keeping forest up to date

```bash
forest self update    # upgrade to latest
```

A one-line nag also prints at the end of every command when a newer release
exists (cached 24h; suppress with `FOREST_NO_UPDATE_CHECK=1` or `CI=true`).

## What's here

- [`apps/forest/`](apps/forest/) — the `forest` CLI and supporting libraries
- [`apps/forage/`](apps/forage/) — the managed web UI ([forage.understory.sh](https://forage.understory.sh))
