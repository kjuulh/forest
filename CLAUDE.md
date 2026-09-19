# forest — usage guide

forest codifies development workflows — CI, deployments, component sharing, and
CLI tool distribution — as [CUE](https://cuelang.org/) manifests you can share
across a team. A `forest.cue` in a repo says what the project is, which components
it depends on, and where it deploys; the `forest` CLI reads that file and does the
rest.

`README.md` covers installing forest and how shell integration works. This file
covers **how to drive it** — the commands to reach for, how to author a
`forest.cue`, and how a release actually flows.

## Orientation

Two file names carry everything, and the distinction matters:

| File | Who writes it | What it declares |
|---|---|---|
| `forest.cue` | every project | project identity, dependencies, commands, where it deploys, and (for a component) how it is built and published |
| `forest.component.cue` | component authors only | the *contract* — `#Spec` (config consumers must supply), `#Commands`, `#Hooks`, `#Tool` |

A repo that only consumes components needs `forest.cue` alone.

`forest --help` lists every command, and `forest <cmd> --help` documents its
flags — the long help is genuinely detailed, so read it rather than guessing.
Two global flags are worth knowing up front:

- **`--format json`** on any list/show command gives typed JSON. Use it whenever
  you're parsing output rather than reading it. `--format name` gives just the
  first column, one per line, for piping into `xargs`.
- `--context <name>` overrides the active server profile for one invocation.

> `forest docs` is advertised in `--help` but panics on 0.3.13 — its own
> `--format` argument collides with the global one. Use `forest <cmd> --help`
> instead until that's fixed.

## Installing and running tools

forest doubles as the org's tool distributor. Tools are components with a `#Tool`
facet; installing one writes a shim and downloads the binary lazily on first use.

```bash
forest global add understory/gitnow          # latest
forest global add understory/awslogin@0.6.1  # pinned
forest global add understory                 # the org's whole catalogue
```

Then `gitnow`/`awslogin` are on `PATH` via `~/.cache/forest/global/shims/`.

| Command | Does |
|---|---|
| `forest global list` | installed tools, their source, version, and cache status |
| `forest global which <tool>` | resolved binary path (cold-fetches if missing) |
| `forest global update` | re-resolve pins and catalogue subscriptions, bump to latest |
| `forest global remove <org>/<name>` | drop a per-tool dependency and its shim |
| `forest global warm` | pre-download binaries so the first real call is instant |
| `forest global verify` | re-hash every cached binary; delete mismatches |
| `forest global sync` | repair shims that drifted from `forest.cue` |

Catalogue subscriptions are tunable per tool without unsubscribing:
`forest global ban <org> <tool>` / `unban`, and `forest global pin <org>/<tool>
<version>` / `unpin`. `add` also accepts `--ban`, `--pin name=version`,
`--alias upstream=local`, and `--as <shim-name>` inline.

User-side state lives in `~/.config/forest/forest.cue`; `forest global add`
maintains it, so hand-editing it means running `forest global sync` afterwards.

### Shell integration

```zsh
eval "$(forest shell zsh)"
```

`bash` and `fish` work the same way. This one line loads the integration for
*every* installed tool, because each tool declares its own script in its component
manifest — there is no per-tool line to add. Optionally `forest shell install`
puts the shim directory on `PATH` in `~/.zshenv`/`~/.bashrc` too, so
non-interactive shells (which skip `~/.zshrc`) can find forest-installed tools;
`forest shell uninstall` reverses it.

Keep forest itself current with `forest self update`.

## Authenticating

```bash
forest auth login       # or `forest auth register`
forest auth status
forest context list     # named server+auth profiles, active one marked *
forest context use understory-prod
```

Contexts work like kubectl contexts. Publishing and releasing both act against
the active one, so check it before anything destructive — publishing to the wrong
registry is the easy mistake here.

## Authoring a forest.cue

### A consuming project

The minimum: identity plus the commands the repo exposes to `forest run`.

```cue
package canopy_example

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "canopy-example"
	organisation: "understory"
	description:  "Example ingest service."
	metadata: {
		git_url: "https://github.com/understory-io/canopy-example"
		owner:   "understory-io"
	}
}

commands: {
	build: ["cargo build --release"]
	test:  ["cargo test --workspace"]
	check: ["cargo clippy --workspace -- -D warnings"]
}
```

`project.name` and `organisation` must both match `^[a-z][a-z0-9-]*$`.
`metadata` is a fixed small set of keys — `git_url`, `homepage`, `docs_url`,
`support_url`, `domain`, `owner`, `tags` — rendered on the project overview.

`commands` maps a name to a list of shell commands run in order; `forest run
build` executes them. Scaffold the file for a fresh checkout with
`forest project init --project-name <name>`, which also writes `cue.mod`.

### Depending on a component

```cue
dependencies: {
	"forest/deployment": version: "0.3.2"          // from the registry
	"forest-contrib/terraform-service": path: "../components/terraform-service"  // local
}
```

Each entry is either `version` or `path` — `path` is how you develop a component
against a consumer before publishing it. `forest add <org>/<name>[@<version>]`
adds an entry for you (`--path` for the local form), and `forest update
[<org>/<name>]` bumps to the latest version matching the spec.

### Saying where it deploys

A component that renders manifests gets a usage block keyed by its
`org` and `name`, with a base `config` and per-environment overrides:

```cue
"forest-contrib": "terraform-service": {
	env: {
		dev: {
			destinations: [{destination: "infrastructure-dev.*", type: "forest/terraform@1"}]
			config: replicas: 2
		}
		prod: {
			destinations: [{destination: "infrastructure-prod.*", type: "forest/terraform@1"}]
			config: replicas: 5
		}
	}

	config: {
		name: "canopy-example"
		ports: [{name: "http", port: 3000, external: true}]
	}
}
```

`destination` is a regex matched against destination names, and `type` names the
destination kind. When the thing being released *already exists* and has no
manifests to render — an ECS service rolled by `forest/generic@1`, say — put `env`
directly on `project` instead, with no component in the picture:

```cue
project: {
	name:         "canopy-schema-applier"
	organisation: "understory"

	env: {
		"data-prod": {
			destinations: [{destination: "^data-prod/.*$", type: "forest/generic@1"}]
			config: service: "canopy_schema_applier"
		}
	}
}
```

`forest destination types` lists the available kinds and `forest destination list
-o <org>` the configured destinations. `forest environment list -o <org>` lists
environments. Run **`forest validate`** after editing any of this — it checks the
project config against the component specs and reports contract gaps.

There are many real `forest.cue` files across the org to copy from — this repo's
`apps/forest/examples/` covers each shape end to end, and
`apps/forest/examples/global-tools/README.md` walks the tool flows. Note that
that README predates the current CLI in places and says `forest components
publish`; the real command is `forest publish`.

### Authoring a component or tool

A component adds a `forest: component:` block to `forest.cue`. How the artifact
gets to users is one of two mutually exclusive keys:

**`upload:`** — forest builds and hosts the binary:

```cue
forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"

	upload: {
		source: "./crates/my-tool"
		type:   "rust"           // rust | go | docker | typescript | deno | prebuilt
		architectures: {
			linux: {amd64: {}, arm64: {}}
			macos: arm64: {}
		}
	}
}
```

A built component only uploads the *host* platform, whatever the matrix says. To
ship binaries a colleague on another OS can run, build them yourself and use
`type: "prebuilt"` with explicit per-platform paths — see `awslogin`'s
`forest.cue` for that pattern.

**`external:`** — the artifact is hosted upstream and forest records only where to
fetch it and what it must hash to. `gitnow`'s own `forest.cue` in this org is a
worked example. Get the hashes with:

```bash
forest tool hash <url> --archive tar.gz --binary-in-archive <path-in-archive>
```

`sha256` is the extracted binary; `archive_sha256` is the tarball. URLs must be
`https://`.

Two optional blocks apply to either shape:

```cue
	// Shipped alongside the binary, materialised into the cache on fetch.
	include: {
		shell: init: {
			zsh: ["init", "zsh"]      // argv run once to capture the integration script
		}
		env: MY_TOOL_MODE: "fast"     // defaults; the ambient environment still wins
	}

	// Checked on PATH before forest dispatches to the component.
	requires: tools: [{name: "cargo", hint: "install rust via rustup"}]
```

Declaring `include.shell.init` is what makes `eval "$(forest shell zsh)"` load
your tool's integration — only list shells the tool can actually emit, since
declaring one it can't just caches a failed capture.

Publishing, from the project directory:

```bash
forest context use understory-prod
forest run build                     # if the component builds its own binary
forest publish --dry-run             # local preflight; prints what would land
forest publish
```

`--dry-run` runs `cue eval`, the binary check, the describe probe, and the
manifest build without contacting the registry — worth doing before every real
publish, since it catches an `external:`/`upload:` mixup and a wrong context.
`--version <v>` (or `$FOREST_COMPONENT_VERSION`) overrides the version from
`forest.cue`, which is how a tag-triggered CI release avoids editing a tracked
file mid-run; export the env form so `forest run build` stamps the same version.

Browse and scaffold with `forest components list -o <org> -q <query>`,
`forest components show <org>/<name>`, and `forest components init <name>`.
Components declaring `codegen` generate typed bindings from the spec with
`forest generate`.

## Releasing

A release moves through three stages. `forest release create` runs all three, and
is what you want most of the time:

```bash
forest release create --environment prod
```

It auto-detects organisation and project from `forest.cue`, and title, commit
SHA, branch, message, and repo URL from git, then waits for the release to
complete.

The stages, if you need them separately:

| Command | Stage |
|---|---|
| `forest release prepare` | invoke component hooks to render deployment manifests |
| `forest release annotate --context-title <title>` | upload artifacts, create the annotation |
| `forest release release` | execute — deploy to destinations |

Flags that come up:

- `-d, --destination` targets specific destinations; omit it and the server decides.
- `--set` overrides a config value for one release without editing `forest.cue` —
  `--set org/component.key=value` for a component's config, `--set config.key=value`
  for a generic destination's deployment block. This is the CI hook for an image
  tag the project can't know when it's written:
  `forest release create -e prod --set config.image_tag=main-601f507`.
- `--detect` works out the release author from the CI environment (GitHub Actions
  payload, then `GITHUB_ACTOR`, then the commit) rather than from whoever is signed
  in — for CI, where a shared token authenticates the annotation.
- `--no-wait` returns without waiting; `--no-health` skips post-release health
  monitoring; `--force` cancels queued releases and jumps the queue.

Observing and steering:

```bash
forest release show                  # interactive picker
forest release show <slug> --follow  # attach to a live release
forest release show <slug> --logs-only > release.log
forest project releases              # current state per destination
```

A plan stage parked awaiting approval is cleared with `forest release approve
[<slug>]` or killed with `forest release reject`. `forest release fail` reports
that a deploy an annotation announced never actually happened.

Guardrails and automation live under `forest project`: `trigger` (release
triggers), `policy` (deployment guardrails), and `pipeline` (release pipelines).

**Releases deploy real infrastructure.** `create`, `release`, `approve`, and
`publish` all take effect immediately against the active context — don't run them
speculatively, and confirm with the user first.

## Where things live

| Path | Contents |
|---|---|
| `~/.config/forest/forest.cue` | user config: global tool pins and catalogue subscriptions |
| `~/.cache/forest/global/shims/` | tool shims on `PATH` |
| `~/.cache/forest/global/shell/` | captured per-tool shell-integration scripts |
| `~/.cache/forest/components/bin/` | global tool binaries, named by sha256 |
| `~/.local/state/forest/forest.lock` | resolved pins, written on first invocation |

Component sources and contexts use the platform directories rather than the XDG
ones — on macOS that's `~/Library/Caches/forest/components/<org>/<name>/<version>`
and `~/Library/Application Support/forest/contexts.json`. Use `forest global
which <tool>` and `forest context list` rather than reading either by hand.

Useful environment variables: `FOREST_SERVER`, `FOREST_CONTEXT`,
`FOREST_NO_SHELL_INTEGRATION=1` (the first thing to try when a new shell
misbehaves), `FOREST_NO_GLOBAL_WARM=1`, `FOREST_NO_AUTO_UPDATE=1`,
`FOREST_NO_UPDATE_CHECK=1`, and `FOREST_LOG`. `-v`/`-vv`/`-vvv` turn structured
logs back on when the rich UI is hiding what happened.

## This repo

- [`apps/forest/`](apps/forest/) — the `forest` CLI and its libraries
- [`apps/forage/`](apps/forage/) — the managed web UI at [forest.understory.sh](https://forest.understory.sh)
- [`apps/forest/examples/`](apps/forest/examples/) — a worked example per project shape
- [`apps/forest/components/forest/sdk/spec.cue`](apps/forest/components/forest/sdk/spec.cue) — the published SDK schema; the authoritative definition of every type above

`apps/forest/cue/forest-sdk/spec.cue` is an older copy of that schema and lags
behind it — read the `components/` one.

`cue` and `gh` must be on `PATH`. Deploying forest and forage is CI's job on push
to `main`; see `README.md`.
