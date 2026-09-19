# Installation

## Private preview

Forest does not yet have a supported anonymous binary distribution. Install it
from the private repository:

```bash
git clone git@git.kjuulh.io:kjuulh/forest.git
cd forest
cargo install --path apps/forest/crates/forest --locked
```

For repository development, use the pinned toolchain:

```bash
cd apps/forest
mise install
cargo build --locked --workspace
```

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

> **Security:** the emitted shell block can start
> `forest global warm --background --quiet`. Warming executes a tool binary to
> capture shell output, caches that output, and later shells source it. Forest
> does not yet require an interactive trust decision for each new digest. Enable
> this only for approved first-party tools. Set
> `FOREST_NO_SHELL_INTEGRATION=1` to disable both sourcing and background warm.

### Tool shell integrations load themselves

For an approved trusted tool, a single line loads its declared shell
integration—completions, wrapper functions, or directory-changing helpers—from
the component manifest.

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

## Server setup

Forest needs a server for authentication, the registry, organisation features,
and releases. For local development:

```bash
cd apps/forest
cp .env.example .env
mise run local:up
mise run dev
```

The development Compose stack starts PostgreSQL, NATS, and MinIO.
`forest-server` applies embedded migrations at startup and listens on
`http://localhost:4040` by default. The `.env.example` values are intentionally
local and insecure; never reuse them outside localhost.

## Configuration

Forest looks for server configuration in this order:

1. `--server` CLI flag
2. `FOREST_SERVER` environment variable
3. Stored credentials from `forest auth login`
