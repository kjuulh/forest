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

Without `@version` it writes the highest stable published version. `@version` is written as given and checked only when a command next loads the project. `forest add` writes `forest.cue` (and a `cue.mod/module.cue` entry), not the lock. A binary download records its version/digest in `forest.lock`; since v0.3.17, forest also records an already-cached binary when the project first resolves it. File/template-only registry dependencies have no version/digest lock.

**Examples:**

```bash
forest add forest-contrib/kubernetes-service
forest add forest-contrib/kubernetes-service@0.2.0
forest add forest-contrib/kubernetes-service --path ../local-dev
```

---

## `forest run build`

There is no `forest build`. A component's build runs as `forest run build`, which dispatches to the build component the project depends on (`forest-contrib/build-rust`, `-go` or `-docker`). That build component writes `.forest/component/output/<os>/<arch>/<name>` for the declared platforms; `FOREST_BUILD_TARGETS=linux/amd64,…` narrows them. A `prebuilt` component builds itself and reads the version to stamp from `forest components version`.

---

## `forest generate`

Generate SDK code from the CUE component spec.

```bash
forest generate --output <DIR> [--language <LANG>]
```

| Option | Description |
|--------|-------------|
| `--output` | Output directory for generated code (default: `codegen.output` from `forest.cue`) |
| `--language` | Target language: `rust`, `typescript`, `deno`, `ts` (auto-detected if omitted) |

---

## `forest publish`

Publish the component to the Forest registry.

```bash
forest publish [--dry-run] [--version <v>]
```

Uploads the built binaries (every platform staged under `.forest/component/output/`), the component's files and its manifest to the effective target registry. It prints `→ Publishing <org>/<name> to <context> (<server>)` (or the server override) before upload. Run `forest run build` before it for built types. Prebuilt dryruns print the target only since v0.3.17; on released v0.3.16 check the context and server overrides yourself.

- `--dry-run` builds the publish manifest without uploading and still needs a login or `FOREST_TOKEN`. Built binary preflight can execute `_meta/describe`, copy the binary into the cache and write `.forest/component/meta.json`; a cached descriptor can be reused. Prebuilt/external shapes skip that probe and those writes.
- The version is `--version` > `FOREST_COMPONENT_VERSION` > `forest.component.version`. `forest components version` prints the result.
- Overwrite is refused while a version is published, prereleases included. Admin unpublish permits reuse, so an exact pin controls selection, not lifetime content identity. Existing binary digest locks reject changed replacement bytes; warm cached binaries are not an online revocation check. Ship a new version instead of reusing one.

---

## `forest validate`

Validate project configuration against component specs.

```bash
forest validate
```

Checks that project config matches component schemas and verifies contract coverage (which deployment hooks are fulfilled).

---

## `forest update`

Refresh binary entries in `forest.lock` to the newest published version matching each dependency's `version:` (`"1.2.3"` exact, `"1.2"`, `"1"` or `"latest"`). Stable versions are preferred; a prerelease is picked only when no stable version matches, or when it is named exactly. `forest.cue` is not rewritten. Other commands resolve a range to a matching binary version lock when present, so a locked binary range moves only when you run `forest update`.

Runtime range resolution and dotted prerelease specs in `forest update` arrived in **v0.3.17**. Released v0.3.16 fails to load project dependency ranges during ordinary commands and rejects dotted RC specs in `update`; exact published versions still work for consumption without `update`.

Lock-first selection applies to registry binary lock entries. File/template-only components have no registry version/digest lock, so their ranges resolve again; choose exact versions for reproducible selection and do not infer binary digest protection for component files.

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
| `--source-username` | Attribute to a specific user |
| `--source-email` | That user's email |
| `--detect` | Work the release author out of the CI environment ([guide](../guides/ci-cd.md#who-a-release-is-attributed-to)) |
| `--run-url` | Link to CI run |
| `--metadata` | Key-value metadata (repeatable) |

### `forest release release`

Execute the release to destinations.

```bash
forest release release [SLUG] [OPTIONS]
```

Select an existing artifact by its annotation slug or `--artifact-id <artifact-uuid>`.
Since v0.3.17, `--ref` / `-r` are rejected during argument parsing, before release
side effects; older CLIs accepted but ignored them. A git branch is not an
artifact identity. This also applies to the `forest release` shorthand.

| Option | Description |
|--------|-------------|
| `--organisation`, `-o` | Organisation name |
| `--project`, `-p` | Project name |
| `--environment`, `-e`, `--env` | Target environment |
| `--destination`, `-d` | Specific destination(s) (repeatable) |
| `--artifact-id`, `--id` | Artifact ID |
| `--force` | Cancel queued releases, jump to front |
| `--pipeline` | Use the project's release pipeline |
| `--no-wait` | Don't stream progress |

The existing `release release --pipeline` path still selects or prompts for an
environment before dispatch. Pass `-e <env>` for noninteractive use; the pipeline
ignores that environment filter and can include prod. This prompt limitation
does not change `create`'s target-only flag contract below.

### `forest release create`

**Since v0.3.17.** Released v0.3.16 does not have
this routing. Check `forest release create --help`: newer source makes
`--environment` optional. A local build may still report `0.3.16`.

Prepare and annotate, then let the project's triggers decide. **Without a
target, triggers remain enabled and may deploy through the whole pipeline,
including prod.** With `--environment` or `--destination`, it releases directly
instead and skips triggers, unless `--skip-commit` is supplied. With `--pipeline`,
the whole pipeline runs regardless of the target environment.

```bash
forest release create [OPTIONS]
forest release create --environment <ENV> [OPTIONS]
```

| Mode | Triggers | Releases |
|------|----------|----------|
| no `--environment` / `--destination` | evaluated | whatever a matching trigger starts, usually the project's pipeline |
| `--environment` or `--destination` | skipped, so they can't race the direct release | that target |
| either, plus `--skip-commit` | evaluated | whatever a matching trigger starts |

Prefer trigger-to-pipeline release for environment ordering and plan approval.
Inspect triggers, the whole pipeline and policies first. Released v0.3.16
servers lack pipeline branch-restriction checks, and their soak can count plans
or pass with missing source success. Server source since v0.3.17 closes those gaps
([Policies](../concepts/policies.md)); updating only the CLI does not.
Review branch, artifact and dev elapsed time before approving prod.
Anchored trigger filters are not authorization for
an explicit pipeline start. Never route around a denied policy with `-d`.

An annotation's default source type is detected from CI or falls back to `local`.
Whether it releases depends on the actual trigger filters, not the machine
that created it. No matching trigger means `show <slug>` finds no release intent.

On v0.3.16 and older, `create` requires an environment,
skips triggers and `--skip-commit` releases nothing. For a version-independent
pipeline path, use `forest release prepare` then `forest release annotate
--source-type ci`. For an annotation without trigger dispatch, use
`annotate --annotation-only` after prepare; there is no `--no-release`.

| Option | Description |
|--------|-------------|
| `--environment`, `-e`, `--env` | Release to this environment directly |
| `--destination`, `-d` | Release to these destinations directly (repeatable) |
| `--skip-commit` | Annotate for the triggers even when a target is given |
| `--pipeline`, `--force`, `--no-wait`, `--no-health` | Apply to the direct release, so they need `--environment` or `--destination` |
| `--title` | Release title (default: latest git commit subject) |
| `--description` | Release description (default: git commit body) |
| `--organisation`, `-o` | Organisation (auto-detected from `forest.cue`) |
| `--project`, `-p` | Project (auto-detected from `forest.cue`) |
| `--commit-sha` | Commit SHA (auto-detected from HEAD) |
| `--source-type` | Source type the triggers match on (default: detected CI, else `local`) |
| `--set` | Override config values (same as `prepare --set`). Overrides are recorded in annotation metadata. |

**CI example** (after the tested image is pushed under an immutable tag):

```bash
forest release prepare --set kjuulh/service.tag="$IMAGE_TAG"
forest release annotate --source-type ci --detect
```

Do not also pass an environment to the release action: that can race the trigger.

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
forest destination update --organisation <ORG> --name <NAME> \
  --metadata "key=value" \
  --sensitive <KEY>
```

| Flag | Meaning |
| --- | --- |
| `--metadata key=value` | Set one metadata key. Repeatable. Keys you do not name are left alone. |
| `--replace-metadata` | Make `--metadata` the destination's *entire* metadata, deleting every key not named. |
| `--sensitive <KEY>` | Replace the destination's sensitive-key set. Repeatable. Omit to leave the set as it is. |
| `--clear-sensitive` | Empty the destination-declared sensitive set. Keys the destination *type* declares sensitive stay hidden regardless. |

Marking a key sensitive withholds its value from `destination list` and from
every API client; the value itself is kept, and deploys still receive it. Read
one back with `forest destination reveal`.

### `forest destination reveal`

Print one withheld metadata value. Deliberately per-key — there is no call that
returns every credential on a destination at once.

```bash
forest destination reveal --organisation <ORG> --name <NAME> --key <KEY>
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
forest notifications listen [--organisation <ORG>] [--project <PROJECT>]
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

## `forest self`

```bash
forest self check [--prerelease]        # exit 1 when a newer release exists
forest self update [--prerelease]       # latest stable, or highest including prereleases
forest self update <version>            # exactly that release, older ones included
```

Since v0.3.17 (check: `forest self update --help` lists `--prerelease`):

- Without a version, `update` never replaces a newer binary with an older
  release. `--prerelease` applies to one run; `FOREST_PRERELEASE=1` applies it
  to `update`, `check` and the update notice while it is set. Nothing is saved.
- A version must be SemVer (`v` optional); it is also the way back to stable.
- The archive's `.sha256` record must name and match it, and the new binary's
  `forest --version` must equal the requested version, before it replaces the
  running one.
- `forest --version` prints `(commit <sha>)` for release builds and
  `(local build)` from source.

Cutting a forest prerelease is described in `RELEASING.md` at the repo root.

## `forest global add --prerelease`

Follow one tool's prereleases instead of its stable releases.

```bash
forest global add understory/fungus --prerelease   # highest SemVer, prerelease or stable
forest global update                               # keeps following prereleases for it
forest global add understory/fungus                # back to the latest stable release
forest global run understory/fungus@1.3.0-rc.1 -- --version   # one build, once
```

Behaviour:

- Without `--prerelease`, `add` and `update` never select a prerelease. A tool
  with no stable release yet is refused with the newest prerelease named, rather
  than installed, and `update` never moves an entry onto a prerelease.
- With it, `add` installs the highest version by SemVer precedence:
  `1.3.0-alpha.10` beats `1.3.0-alpha.2`, `1.3.0` beats `1.3.0-rc.2`, and build
  metadata (`1.3.0+build.5`) is not a prerelease. The choice is stored as
  `prerelease: true` on that entry in `~/.config/forest/forest.cue`, so `forest
  global update` and the daily background update follow it. `forest global list`
  shows such a tool as `[prerelease]`.
- Running `add` again without the flag returns the tool to stable and clears the
  opt-in; if the tool has no stable release, the opt-in is left as it was.
  Hand-set `env` overrides on the entry are kept either way; `--as` must be
  passed again.
- The opt-in is per tool. Org catalogue subscriptions stay on stable releases,
  and a per-tool entry shadows the catalogue entry as it always has.
- `FOREST_PRERELEASE=1` in the environment follows prereleases for every tool
  that is not pinned, catalogue tools included, for as long as it is set:
  `add` without the flag, `update` and the background update, and
  `run`/`which`/`warm` resolution. It is not written to `forest.cue`. Unset it
  (or set `0`) and the next `update` returns per-tool entries to stable;
  catalogue tools resolve to stable straight away. Catalogue pins, bans and
  `@<version>` pins still win. A tool with only prereleases is not in the org
  catalogue, so it still has to be added per tool. Put it in `~/.zshenv` (or
  your shell's equivalent), not only `~/.zshrc`, so shims run from scripts and
  the background update see it too.
- `@<version>` is an exact pin, prerelease or not, and `update` holds it. It
  cannot be combined with `--prerelease`, nor can a whole-org `add <org>`.
- `forest global run`/`which` with `<org>/<name>@<version>` runs that build
  without changing the installed selection.
- Overwrite of a currently published version, prerelease included, is refused.
  Ship a new build as a new prerelease number (`1.3.0-rc.2`). Admin unpublish
  permits reuse of the number, so exact pins control selection, not lifetime
  content identity. The lockfile keeps the digest first fetched and refuses
  replacement bytes on a later fetch. A warm cached binary can continue running
  without an online revocation check.
  Since v0.3.17 it also covers forest itself; see [`forest self`](#forest-self).

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
