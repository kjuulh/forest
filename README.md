# Forest - developer platform

Codify your development workflows; CI, deployments, component sharing as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

*Prerequisites*

- `gh` GitHub cli
- `cue` Cuelang (yaml and golang had a baby)

To install forest run the below command, it will install forest as a cli, and set your current profile to run against the production instance of forest.

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - \
  | FOREST_PROFILE='name=understory-prod,server=https://api.forest.understory.sh' bash
```

Next you need to add it to `.zshrc` to get full cli support

```bash
echo 'eval "$(forest shell zsh)"' >> ~/.zshrc
```

Optionally run `forest shell install` to put forest's global tools on your
`PATH` so you can run them directly (reverse with `forest shell uninstall`).

### Tool shell integrations load themselves

You don't need a line per tool in your `.zshrc`. A tool that ships shell
integration — completions, a wrapper function, a `cd`-ing helper — declares it in
its own component manifest, and the single `eval "$(forest shell zsh)"` above
loads all of them.

Component side, in the tool's `forest.cue`:

```cue
forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.5.0"

	// Forest runs `awslogin shell <shell>` once when the tool is fetched,
	// caches the output, and serves it from `forest shell <shell>`.
	include: shell: init: {
		zsh: ["shell", "zsh"]
		bash: ["shell", "bash"]
		fish: ["shell", "fish"]
	}
}
```

User side: nothing. New shells pick it up.

**Why this exists.** Global tools install *lazily* — a shim downloads its binary
the first time it runs. That is the right trade for `gitnow status`, but an rc
file full of

```zsh
eval "$(gitnow init zsh)"      # ← on a cold cache each of these downloads a
eval "$(awslogin shell zsh)"   #   multi-MB binary before your prompt appears
```

turns a cold cache into a serial download queue with your prompt stuck behind it.
Declaring the integration inverts that: forest captures each script *once*, when
the tool is fetched, and concatenates them into one cached file. Shell startup
became a single file read — no process per tool, and no download on the critical
path ever.

| | cold cache, first shell | warm cache |
|---|---|---|
| `eval "$(<tool> …)"` per tool | 2.2–3.4 s | 65 ms |
| component-declared | **46–58 ms** | **20 ms** |

On a cold cache the prompt appears immediately, a detached warm downloads the
tools, and the integrations load into the shell you're already sitting in as soon
as they land.

Useful knobs:

- `forest global warm` — foreground warm with progress; worth running after
  `forest global update` bumps versions.
- `forest global warm --background --quiet` — what the emitted block calls: it
  detaches, prints nothing, and is throttled so opening ten terminals costs one
  warm.
- `FOREST_NO_SHELL_INTEGRATION=1` turns the whole thing off — nothing sourced, no
  warm started, the rest of your rc file untouched. Reach for this first if a new
  shell starts misbehaving: the block sources each tool's own integration script,
  so setting this and opening a new shell tells you in one step whether forest is
  involved.
- `FOREST_NO_GLOBAL_WARM=1` disables background warming;
  `FOREST_GLOBAL_WARM_INTERVAL_SECS` overrides the 30-minute throttle.
- `forest-init <tool> <args…>` — the escape hatch for the two cases forest can't
  discover: a tool that isn't a forest component (installed via cargo, brew, …),
  or a forest tool whose component hasn't declared `include.shell` yet. It
  replaces `eval "$(<tool> …)"` without ever blocking a cold shell:

  ```zsh
  eval "$(forest shell zsh)"       # defines forest-init — must come first
  forest-init kignore init zsh     # cargo-installed, not a forest component
  ```

bash and fish work the same way via `forest shell bash` / `forest shell fish`.

## Logging in

Either create an account or sign in, both can be done entirely in the terminal if wanted

```bash
forest auth login
```

### Keeping forest up to date

```bash
forest self update    # upgrade to latest
```

A one-line nag also prints at the end of every command when a newer release
exists (cached 24h; suppress with `FOREST_NO_UPDATE_CHECK=1` or `CI=true`).

## What's here

- [`apps/forest/`](apps/forest/) — the `forest` CLI and supporting libraries
- [`apps/forage/`](apps/forage/) — the managed web UI ([forest.understory.sh](https://forest.understory.sh)). Directory name remains `forage` for now; the crate hasn't been renamed.
