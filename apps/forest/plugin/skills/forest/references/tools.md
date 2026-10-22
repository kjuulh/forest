# Shared tools: `forest global`

Org CLIs (gitnow, awslogin, fungus, …) are components with a `#Tool` facet. `forest global add` records a selection in `~/.config/forest/forest.cue` and writes a shim. The binary downloads lazily on first use, is checked against a locked sha256, and is cached by digest. `forest global` is hidden from `forest --help`, so use `forest global --help`.

## Find, install, run

List installed selections with `global list`; discover registry versions with `components show`. For an intentional RC trial on any CLI version, use an exact selector:

```bash
forest global list
forest components show <org>/<name>
forest global run <org>/<name>@1.3.0-rc.1 -- --version
```

This runs once without changing the installed selection. Persist it only when requested: `forest global add <org>/<name>@1.3.0-rc.1`. To discover names, use `forest components list -o <org> -q <q>`; `global which <tool>` downloads if missing. Shims require the shim directory on `PATH`; explicit `global run` does not.

Prefer an existing org tool over a hand-written script or a manually installed binary; check `forest global list` and the registry first. A tool's own `--help` is the reference for its subcommands.

Forest's own flags go before `--`, and everything after it goes to the tool. `forest global run org/name@1.2.3 -- --version` runs that exact build **once** without changing the selection, which is the safe way to try a version.

## How a version is chosen

This table describes v0.3.15 and newer. On v0.3.14 and older the RC-only fallback differs and `--prerelease` is unavailable; see the version boundary below before choosing a floating selection.

| Form | Picks | `forest global update` |
|---|---|---|
| `add <org>/<name>` | highest stable | bumps to the newest stable |
| `add <org>/<name>@<v>` | exactly `<v>`, prerelease or not (`[pinned]`) | holds it |
| `add <org>/<name> --prerelease` | highest SemVer including prereleases (`[prerelease]`) | follows prereleases; a final release beats its own RCs |
| `add <org>` (catalogue) | each tool's latest stable, resolved live at run time | n/a, since nothing is stored |
| `run`/`which <org>/<name>@<v>` | exactly `<v>`, this once | n/a |
| any unpinned tool while `FOREST_PRERELEASE=1` (v0.3.16+) | highest SemVer including prereleases, catalogue tools included | follows prereleases while set |

- There are **no ranges** for tools: `@0.2` and `@^0.2` fail with `manifest not found for <org>/<name>@0.2`.
- A per-tool entry shadows the same tool in a catalogue subscription. Tune a catalogue with `forest global ban|unban <org> <tool>`, `forest global pin|unpin <org>/<tool> [<version>]` and `add <org> --alias upstream=local`. `pin` does not check that the version exists. `--as <shim-name>` renames a per-tool shim, and it must be passed again on every re-`add`.
- `forest global update` runs in the background about once a day, but only when a shim or `forest global run` is invoked from a terminal. It is skipped with `FOREST_NO_AUTO_UPDATE=1`, with `CI` set, or when stderr is not a TTY, so agents and scripts never trigger it.

## Tool prereleases: release status matters

`--prerelease` shipped in **v0.3.15** (released 2026-10-20; #324). Users on v0.3.14 or older don't have it until they run `forest self update`. Check before relying on it:

```bash
forest global add --help | grep -- --prerelease
```

**v0.3.15 and newer:**

- Without `--prerelease`, `add` and `update` never pick a prerelease. A tool with only prereleases is refused with a message naming its newest RC, `--prerelease` and `@<version>`, and `update` leaves such entries alone.
- `add <org>/<name> --prerelease` stores `prerelease: true` on that entry. Running `add` again without the flag returns to the newest stable and clears the opt-in; if the tool has no stable release, the opt-in is left as it was.
- `@<v> --prerelease` and `<org> --prerelease` are rejected.

**v0.3.16 and newer:** `FOREST_PRERELEASE=1` (truthy; `0`/`false`/`no`/`off` means off) makes every tool that isn't pinned resolve to its highest SemVer version, prerelease or stable, while it is set:

- `add` without the flag, `update` and the background update;
- catalogue tools in `list`, `run`, `which` and `warm`.

It is read at resolution time and never written to `forest.cue`. Unset it to stop: per-tool entries return to stable on the next `update`, and catalogue tools resolve to stable straight away. Catalogue pins, bans and `@<version>` pins still win. A tool with only prereleases is not in the org catalogue, so it still has to be added per tool. Set it in `~/.zshenv` (or your shell's equivalent), not only in `~/.zshrc`, so that shims run from scripts and the background update see it too. It is not an agent default: set it only when the user wants every tool on prereleases. After v0.3.16 it also makes `forest self update`, `self check` and the update notice consider forest's own prereleases; see [setup-and-context.md](setup-and-context.md).

**Released v0.3.14 and older:**

- There is no `--prerelease` flag (`unexpected argument`).
- `add` on a tool with only prereleases silently installs its newest RC as an ordinary floating entry, and `update` moves it from RC to RC until a stable release exists.
- Use an exact `@<rc>` pin for an intentional persistent selection; `global run org/name@<rc> -- …` remains available for a one-off trial.
- An older CLI ignores the `prerelease: true` key a newer one wrote, and drops it on its next config write.

To ship a fix to testers: publish `1.3.0-rc.1`, then have them run `forest global add <org>/<name>@1.3.0-rc.1`, or `--prerelease` on a CLI that has it. Use `rc.2`, `rc.3` and so on for rebuilds. Currently published versions reject overwrite, but an admin unpublish permits reuse; exact pins control version selection, not lifetime content identity.

## Integrity

- `~/.local/state/forest/forest.lock` holds one `org/name@version os/arch sha256:…` line per version used. Downloads must match it; on a mismatch the server returns `DigestChanged` or the client refuses with `sha mismatch — refusing to write to cache`.
- Forest **never re-pins a digest automatically.** If the same version's bytes changed (an admin unpublished and republished it), stop and ask the tool's owner. Don't delete lock lines to make the error go away, because that defeats the check. The supported way forward is a new version.
- A warm cached binary can remain usable after unpublish; running it is not an online revocation check. If the version is reused, an existing digest lock detects changed bytes when fetched rather than silently accepting a replacement.
- `forest global verify` re-hashes the cache and deletes mismatched binaries; it does not touch the lockfile. `forest global sync` repairs shims after a hand edit of `~/.config/forest/forest.cue`.

## Shell integration

```zsh
eval "$(forest shell zsh)"   # bash / fish likewise; fish: `forest shell fish | source`
forest shell install         # optional: also put shims on PATH for non-interactive shells
```

This sources every installed tool's declared `include.shell.init` script from one cached file. Snippets are captured by `forest global warm`, not by `update`, so a freshly updated tool may use its previous snippet until the next warm. `FOREST_NO_SHELL_INTEGRATION=1` turns the whole block off, which is the first thing to try when a new shell misbehaves. `forest-init <cmd> <args>` defers `eval "$(<tool> init zsh)"` for tools Forest doesn't manage.

## Failure modes

| Symptom | Meaning | Next step |
|---|---|---|
| `<org>/<name> has no stable release yet; its newest version is the prerelease …` | prerelease-only tool, newer CLI | `--prerelease` or `@<rc>` if the user wants it |
| `error: unexpected argument '--prerelease'` | CLI is v0.3.14 or older | `@<rc>`, or `forest self update` (v0.3.15+) if the user agrees |
| `not found: <org>/<name>` / `manifest not found for <org>/<name>@<v>` | wrong name or org, a range like `@1.2`, an unpublished version, or no access in this context | `forest components list -o <org>`, `forest context active` |
| `tool … not available for linux/amd64; published for: darwin/arm64` | the publisher shipped one platform | the publisher must ship that platform |
| shim prints `exec: forest: not found` | `forest` itself is not on `PATH` in that shell | fix `PATH`; shims re-enter the CLI |
| exit 75 | `--no-fetch` / `FOREST_GLOBAL_NO_FETCH` and not cached yet | run it normally, or `forest global warm` |
| unexpected version | a per-tool entry, pin or catalogue is shadowing another | `forest global list`, `forest global which <tool>` |
