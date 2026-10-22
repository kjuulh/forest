# Installation

## From a Release

Releases are published on the private `understory-io/forest` GitHub repository,
so `gh` must be installed and logged in (`gh auth status`). This installs the
latest stable release to `~/.local/bin`:

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - | bash
```

To install one exact release, prerelease or not, pass its tag as listed by
`gh release list --repo understory-io/forest`:

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - | bash -s -- <tag>
```

The installer verifies the tarball's SHA-256 and refuses a binary whose
`forest --version` is not the version asked for.

## Updating

`forest self update` installs the latest release and `forest self check` exits
1 when one is newer. The rest of this section describes forest **after
v0.3.16**; check that `forest self update --help` lists `--prerelease`. On
v0.3.16 and older, plain `self update` installs GitHub's latest release even
over a newer binary, and `forest self update <tag>` installs an exact release.

| Command | Installs |
|---|---|
| `forest self update` | the latest stable release; leaves a newer binary (a prerelease, a local build) in place |
| `forest self update --prerelease` | the highest SemVer release, stable or prerelease, with an archive and checksum for this platform; for this run only |
| `forest self update <version>` | exactly that release, older ones included |
| `forest self check [--prerelease]` | nothing; exit 1 when a newer release exists |
| `FOREST_PRERELEASE=1 forest self update` | as `--prerelease`, for every run while the variable is set |

Prereleases rank by SemVer, for example `1.4.0-alpha.2` < `1.4.0-alpha.10` <
`1.4.0-rc.1` < `1.4.0`, so a final release replaces its own release
candidates. Nothing is written to config. To always take prereleases, set
`FOREST_PRERELEASE=1` in your shell profile: the same variable makes every
unpinned global tool follow its prereleases, and with it set `self update`,
`self check` and the background update notice consider forest's prereleases
too. Unset it (or set `0`) to go back; the notice re-checks at once.

Unsetting it does not step an installed prerelease down: a plain
`self update` leaves a binary newer than the latest stable release in place.
To go back to that stable release, name it: `forest self update v<latest-stable>`.
Every path verifies the checksum, then that the new binary reports the
requested version, before replacing the running one.

`forest --version` prints the commit a release was built from, as
`forest <version> (commit <sha>)`; a binary built from source says
`(local build)`. Releases up to and including v0.3.16 print only
`forest <version>`.

This is separate from `forest global add <org>/<tool> --prerelease`, which
makes a tool installed *through* forest follow that tool's prereleases.

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

## Shell Integration

Add the integration to your interactive rc file so your shell finds forest's
global tools:

```bash
echo 'eval "$(forest shell zsh)"' >> ~/.zshrc    # or: forest shell bash
```

For fish, source it from your config instead:

```fish
echo 'forest shell fish | source' >> ~/.config/fish/config.fish
```

Optionally run `forest shell install` to put forest's global tools on your
`PATH` so you can run them directly (reverse with `forest shell uninstall`).

### Tool shell integrations load themselves

The single line above is all you need. A tool that ships shell integration —
completions, a wrapper function, a `cd`-ing helper — declares it in its own
component manifest, and `forest shell zsh` loads every installed tool's.

```cue
// in the tool's own forest.cue
forest: component: sdk.#ForestComponent & {
	include: shell: init: {
		zsh: ["shell", "zsh"]
		bash: ["shell", "bash"]
		fish: ["shell", "fish"]
	}
}
```

Forest runs that command once when the tool is fetched, caches the output, and
concatenates every tool's script into one file that shell startup sources. See
[authoring components](../guides/authoring-components.md#shipping-shell-integration)
for the component side.

**Why it works this way.** Global tools install *lazily* — a shim downloads its
binary on first use. So an rc file that evals tools purely for their init scripts

```zsh
eval "$(gitnow init zsh)"      # ← downloads a multi-MB binary just to print
eval "$(awslogin shell zsh)"   #   an init script, on every fresh machine
```

turns a cold cache into a serial download queue in front of your prompt. Capturing
the script at fetch time removes both the download *and* the per-tool process:

| | cold cache, first shell | warm cache |
|---|---|---|
| `eval "$(<tool> …)"` per tool | 2.2–3.4 s | 65 ms |
| component-declared | 46–58 ms | 20 ms |

On a cold cache the prompt appears immediately, a detached warm fetches the tools,
and the integrations load into the shell you're already in as soon as they land.

| Knob | Effect |
|---|---|
| `forest global warm` | Foreground warm with progress. Worth running after `forest global update`. |
| `forest global warm --background --quiet` | What the emitted block calls: detached, silent, throttled. |
| `FOREST_NO_SHELL_INTEGRATION=1` | Load nothing, start no warm. The escape hatch: try this first if a new shell misbehaves — it settles in one step whether forest is involved. |
| `FOREST_NO_GLOBAL_WARM=1` | Disable background warming entirely. |
| `FOREST_GLOBAL_WARM_INTERVAL_SECS=<n>` | Override the 30-minute warm throttle. |
| `forest-init <tool> <args…>` | Escape hatch for tools forest can't discover — non-forest installs (cargo, brew), or components that haven't declared `include.shell` yet. Never blocks a cold shell. |
| `FOREST_GLOBAL_NO_FETCH=1` | What `forest-init` sets: make a shim skip (exit 75) rather than download. |

bash and fish work the same way via `forest shell bash` / `forest shell fish`.

## Requirements

- **Rust 1.98.1+** — Forest uses recent Rust features
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
