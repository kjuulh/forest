# CLI Reference

Complete reference for the `forest` command-line tool.

## Global Options

| Option | Description |
|--------|-------------|
| `--version` | Print version |
| `--help` | Print help |

---

## `forest init`

Scaffold a new project or component from a starter template.

```bash
forest init [STARTER] [--dest <PATH>]
```

| Argument | Description |
|----------|-------------|
| `STARTER` | Starter template name (optional, prompts if omitted) |
| `--dest`, `--destination` | Target directory (default: `.`) |

---

## `forest add`

Add a component dependency to the project.

```bash
forest add <COMPONENT> [--path <PATH>]
```

| Argument | Description |
|----------|-------------|
| `COMPONENT` | Component reference: `org/name` or `org/name@version` |
| `--path` | Use a local path instead of registry version |

**Examples:**

```bash
forest add forest-contrib/kubernetes-service
forest add forest-contrib/kubernetes-service@0.2.0
forest add forest-contrib/kubernetes-service --path ../local-dev
```

---

## `forest build`

Build the component binary for all configured platforms.

```bash
forest build
```

Reads `forest.cue` and `spec.cue` to determine component name, version, and target architectures. Outputs binaries to `~/.cache/forest/components/bin/`.

---

## `forest generate`

Generate SDK code from the CUE component spec.

```bash
forest generate --output <DIR> [--language <LANG>]
```

| Option | Description |
|--------|-------------|
| `--output` | Output directory for generated code (required) |
| `--language` | Target language: `rust`, `typescript`, `deno`, `ts` (auto-detected if omitted) |

---

## `forest publish`

Publish the component to the Forest registry.

```bash
forest publish
```

Uploads the compiled binary, CUE spec files, and component manifest. Requires `forest build` to be run first.

---

## `forest validate`

Validate project configuration against component specs.

```bash
forest validate
```

Checks that project config matches component schemas and verifies contract coverage (which deployment hooks are fulfilled).

---

## `forest update`

Update dependencies to the latest versions matching the spec.

```bash
forest update [COMPONENT]
```

| Argument | Description |
|----------|-------------|
| `COMPONENT` | Specific component to update (`org/name`). If omitted, updates all. |

---

## `forest run`

Run a project or component command.

```bash
forest run <COMMAND> [ARGS...]
```

Commands are dynamically discovered from component definitions. Supports both short and qualified names:

```bash
forest run status               # Short name
forest run my-component:status  # Fully qualified
```

All trailing arguments are passed as `--key value` pairs to the component.

**Special value syntax:**

| Syntax | Description |
|--------|-------------|
| `--key value` | Pass a literal string value |
| `--key @-` | Read value from stdin |
| `--key @/path/to/file` | Read value from a file |
| `--flag` | Boolean flag (no value — sets to `true`) |

**Examples:**

```bash
# Pass a literal value
forest run seal --env dev --key MY_SECRET --value "my-value" --cert cert.pem

# Read value from stdin (useful for multi-line content like credentials)
cat /path/to/creds.txt | forest run seal --env dev --key NATS_CREDS --value @- --cert cert.pem

# Read value from a file
forest run seal --env dev --key NATS_CREDS --value @/path/to/creds.txt --cert cert.pem
```

---

## `forest release`

Manage the release lifecycle.

### `forest release prepare`

Generate deployment manifests by invoking component hooks.

```bash
forest release prepare [--set KEY=VALUE ...]
```

| Option | Description |
|--------|-------------|
| `--set` | Override config values. Format: `org/component.key=value`. Repeatable. |

**Examples:**

```bash
# Pin an image tag from CI
forest release prepare --set kjuulh/service.tag=abc123

# Override multiple values
forest release prepare \
  --set kjuulh/service.tag=abc123 \
  --set kjuulh/service.env_vars.LOG_LEVEL=debug
```

The `--set` flag overrides values in the component's `config` block without modifying `forest.cue`. This is designed for CI pipelines where the image tag is determined at build time.

### `forest release annotate`

Upload artifacts and create a release annotation. Commit SHA and branch are auto-detected from git if not specified.

```bash
forest release annotate [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--organisation`, `-o` | Organisation name (required) |
| `--project-name` | Project name (required) |
| `--context-title` | Release title (required) |
| `--context-description` | Release description |
| `--context-web` | Web link to the change |
| `--context-pr` | Pull request link |
| `--commit-sha` | Commit SHA (auto-detected from git HEAD) |
| `--commit-branch` | Source branch (auto-detected from git) |
| `--commit-message` | Commit message |
| `--source-type` | Source type (e.g., `ci`, `manual`) |
| `--source-username` | Who triggered the release |
| `--source-email` | Triggerer's email |
| `--run-url` | Link to CI run |
| `--metadata` | Key-value metadata (repeatable) |

### `forest release release`

Execute the release to destinations.

```bash
forest release release [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--organisation`, `-o` | Organisation name |
| `--project`, `-p` | Project name |
| `--environment`, `-e`, `--env` | Target environment |
| `--destination`, `-d` | Specific destination(s) (repeatable) |
| `--ref`, `-r` | Artifact reference |
| `--artifact-id`, `--id` | Artifact ID |
| `--force` | Cancel queued releases, jump to front |
| `--pipeline` | Use the project's release pipeline |
| `--no-wait` | Don't stream progress |

### `forest release create`

Combined command: prepare, annotate (without triggers), and release.

```bash
forest release create --environment <ENV> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--environment`, `-e`, `--env` | Target environment (required) |
| `--title` | Release title (default: latest git commit subject) |
| `--description` | Release description (default: git commit body) |
| `--organisation`, `-o` | Organisation (auto-detected from `forest.cue`) |
| `--project`, `-p` | Project (auto-detected from `forest.cue`) |
| `--commit-sha` | Commit SHA (auto-detected from HEAD) |
| `--set` | Override config values (same as `prepare --set`). Overrides are recorded in annotation metadata. |

**CI Example:**

```bash
# Build image, get tag, release with pinned tag
IMAGE_TAG=$(git rev-parse --short HEAD)
docker build -t my-registry/my-app:$IMAGE_TAG .
docker push my-registry/my-app:$IMAGE_TAG

forest release create --env dev --set kjuulh/service.tag=$IMAGE_TAG
```

---

## `forest project`

Manage projects.

### `forest project create`

```bash
forest project create --organisation <ORG> --name <NAME>
```

### `forest project init`

Initialize from `forest.cue`.

```bash
forest project init
```

### `forest project publish`

Publish project configuration.

```bash
forest project publish
```

### `forest project list`

```bash
forest project list --organisation <ORG>
```

### `forest project releases`

Show current release state per destination.

```bash
forest project releases --organisation <ORG> --project <PROJECT>
```

### `forest project trigger`

Manage release triggers. Subcommands: `create`, `list`, `update`, `delete`.

### `forest project policy`

Manage deployment policies. Subcommands: `create`, `list`, `update`, `delete`, `evaluate`.

### `forest project pipeline`

Manage release pipelines. Subcommands: `create`, `list`, `update`, `delete`.

---

## `forest destination`

Manage deployment destinations.

### `forest destination create`

```bash
forest destination create --organisation <ORG> --name <NAME> --environment <ENV> --type <TYPE>
```

### `forest destination update`

```bash
forest destination update --organisation <ORG> --name <NAME>
```

### `forest destination delete`

```bash
forest destination delete --organisation <ORG> --name <NAME>
```

### `forest destination list`

```bash
forest destination list --organisation <ORG>
```

### `forest destination types`

List available destination types.

```bash
forest destination types
```

---

## `forest environment`

Manage environments.

### `forest environment create`

```bash
forest environment create --organisation <ORG> --name <NAME>
```

### `forest environment list`

```bash
forest environment list --organisation <ORG>
```

### `forest environment get`

```bash
forest environment get --organisation <ORG> --name <NAME>
```

### `forest environment update`

```bash
forest environment update --organisation <ORG> --name <NAME>
```

### `forest environment delete`

```bash
forest environment delete --organisation <ORG> --name <NAME>
```

---

## `forest organisation`

Manage organisations and members.

```bash
forest organisation list
forest organisation members --organisation <ORG>
```

---

## `forest components`

Browse and manage components in the registry.

### `forest components init`

Scaffold a new component from a template.

```bash
forest components init <NAME> [--organisation <ORG>] [--language <LANG>] [--output <DIR>]
```

| Option | Default | Description |
|--------|---------|-------------|
| `NAME` | | Component name (required) |
| `--organisation` | `forest-contrib` | Organisation namespace |
| `--language` | `rust` | Implementation language |
| `--output` | `.` | Output directory |

### `forest components list`

Search and list components in the registry.

```bash
forest components list
```

---

## `forest auth`

Authentication commands.

```bash
forest auth register    # Create a new account
forest auth login       # Authenticate with the server (interactive picker; web or password)
forest auth logout      # Log out
forest auth status      # Show current auth status
forest auth token       # Manage personal access tokens
```

### `forest auth login` modes

The interactive picker (TTY default) offers two paths; flags select non-interactively:

| Flag | Behaviour |
|---|---|
| `--web` | Open browser at the active context's forage URL, display a one-time code, poll for approval (RFC 8628 device authorization grant). Default when stdin/stderr is not a TTY. |
| `--password` | Legacy username/email + password flow. MFA challenge in the terminal. Required when piping `FOREST_PASSWORD` from a script. |
| `--username <u>` / `--email <e>` | Imply `--password`. Mutually exclusive with `--web`. |

Configuring where the browser opens (web flow):

1. The context's `web_url` field (`forest context set-web-url <name> <url>`).
2. `FOREST_WEB_URL=…` for a per-invocation override.
3. Convention: `forest.X` → `forage.X`; `localhost:4040` → `localhost:3000`.

---

## `forest notifications`

Listen for and manage notifications.

```bash
forest notifications subscribe [OPTIONS]
```

---

## `forest shell`

Shell integration for the global-tools shim dir.

```bash
forest shell zsh          # emit integration to source from ~/.zshrc (or: bash, fish)
forest shell install      # put the shim dir on PATH so tools run directly
forest shell uninstall    # remove what `install` wrote
```

Fish sources it from `~/.config/fish/config.fish`:

```fish
forest shell fish | source
```

`install` is an optional convenience that puts forest's global tools on your
`PATH`. It's idempotent and reversible.

### What the emitted block does

`forest shell <shell>` emits three things:

1. The idempotent shim-dir `PATH` prepend.
2. The helper functions (`forest-tmp`, `forest-init`, the deferred loader).
3. A block that sources the **shell-integration aggregate** — every installed
   tool's component-declared integration, concatenated into one cached file
   (`$XDG_CACHE_HOME/forest/global/shell/<shell>.sh`).

Tools declare their own integration with `include.shell.init.<shell>` in the
component manifest; forest captures the output once when the tool is fetched. So
shell startup is a single file read: no process per tool, and no lazy download on
the critical path.

If the aggregate doesn't exist yet (fresh install, cold cache), the block starts a
detached silent warm and arms a prompt hook that sources the aggregate the moment
it appears — the integrations land in the shell you're already in, without ever
having blocked it.

### Turning it off

```bash
export FOREST_NO_SHELL_INTEGRATION=1
```

Nothing is sourced and no warm is started, in all three shells. The rest of your rc
file is untouched and `forest` stays on `PATH`.

Reach for this first if a new shell starts misbehaving, because the block sources
script forest did not write — each tool's own integration, concatenated. Setting it
and opening a new shell tells you in one step whether forest is involved: if the
problem goes away it is ours, and if it doesn't, it isn't. That beats bisecting
your rc file.

### `forest-init` — the escape hatch

For tools forest can't discover: ones that aren't forest components (installed via
cargo, brew, …), or forest tools whose component hasn't declared `include.shell`
yet.

```zsh
eval "$(forest shell zsh)"       # defines forest-init — must come first
forest-init kignore init zsh     # cargo-installed, not a forest component
```

It replaces `eval "$(<tool> <args…>)"` without blocking a cold shell: the tool runs
with `FOREST_GLOBAL_NO_FETCH=1`, so a forest shim reports "not cached yet" (exit
`75`, `EX_TEMPFAIL`) instead of downloading at startup. Skipped integrations are
queued and retried from a `precmd` hook (zsh) / `PROMPT_COMMAND` (bash) /
`fish_prompt` event (fish). Cached tools and non-forest commands take the ordinary
path — one exec, `eval` the output.

---

## `forest global warm`

Pre-download the binaries for global tools that aren't cached yet, capture their
declared shell integrations, and rebuild the per-shell aggregate.

```bash
forest global warm                          # foreground, with progress
forest global warm gitnow awslogin          # only these (shim or <org>/<name>)
forest global warm --background --quiet     # detach, print nothing — for rc files
forest global warm --background --force     # ignore the throttle
```

Behaviour:

- **`--background`** returns immediately and does the work in a detached child
  with `/dev/null` stdio, so it is safe to call from a shell rc file. Throttled
  to one warm per 30 minutes (`FOREST_GLOBAL_WARM_INTERVAL_SECS` overrides), and
  the slot is claimed atomically before spawning — a burst of terminals produces
  one warm, not one per terminal.
- **`--quiet`** silences the download narration too, not just the summary, so it
  is genuinely safe on the shell-startup path.
- A **single-instance lock** means two warms never download the same tool at
  once, even with `--force`.
- Already-cached tools are skipped, so a repeat warm costs a lockfile read — but
  their **shell snippets are still captured** if missing, which is how a tool
  cached before it declared `include.shell` catches up.
- The aggregate is rebuilt **by warm only**. `forest global update` deliberately
  leaves it alone: snippets are keyed by version, so a bump means the new version
  has nothing captured yet, and rebuilding there removed the tool's integration
  from every new shell until something happened to run a warm. Since `update` also
  runs from the daily background auto-update, that happened unattended.
- A tool whose installed version has no captured snippet falls back to its newest
  captured one rather than dropping out. The entry is annotated with the version it
  came from, and the next warm refreshes it.
- Per-tool failures are reported and don't stop the rest of the toolset.
- `FOREST_NO_GLOBAL_WARM=1` disables warming entirely, including the implicit
  warms that a cold shell start or a skipped `forest-init` would trigger.

### Capturing a declared integration

For each tool with `include.shell.init.<shell>` in its manifest, warm runs
`<binary> <argv>` once per shell and caches stdout at
`components/include/<org>/<name>/<version>/shell/<shell>.sh`. The capture is
bounded — stdin is closed, output is capped at 512 KB, and the child is killed
after 10 s — because it executes third-party code during a warm. Failures are
per-shell and logged at `debug`; they never fail the warm.
