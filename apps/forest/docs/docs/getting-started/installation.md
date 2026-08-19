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
| `FOREST_NO_GLOBAL_WARM=1` | Disable background warming entirely. |
| `FOREST_GLOBAL_WARM_INTERVAL_SECS=<n>` | Override the 30-minute warm throttle. |
| `forest-init <tool> <args…>` | Escape hatch for tools forest can't discover — non-forest installs (cargo, brew), or components that haven't declared `include.shell` yet. Never blocks a cold shell. |
| `FOREST_GLOBAL_NO_FETCH=1` | What `forest-init` sets: make a shim skip (exit 75) rather than download. |

bash and fish work the same way via `forest shell bash` / `forest shell fish`.

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
