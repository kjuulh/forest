# Setup, contexts and credentials

## Install and keep current

Forest is distributed from the private `understory-io/forest` GitHub releases, so `gh` must be authenticated. `cue` must be on `PATH` too: Forest shells out to it and does not vendor it.

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - \
  | FOREST_PROFILE='name=<context>,server=<server-url>' bash
forest self check      # is a newer stable release out?
forest self update     # install it
```

`install.sh` seeds the context from `FOREST_PROFILE` with `forest context provision`. Installing or updating the user's CLI is their call; suggest the command rather than running it unasked.

**After v0.3.16** (check: `forest self update --help` lists `--prerelease`), forest itself has prereleases:

- `forest self update` takes the latest stable release, and never replaces a newer binary (a prerelease, a local build) with an older one.
- `forest self update --prerelease` takes the highest SemVer release, stable or prerelease, for that run; `forest self check --prerelease` compares against it. `alpha.2 < alpha.10 < rc.1 < final`, so a final release replaces its own RCs.
- `FOREST_PRERELEASE=1` makes `self update`, `self check` and the update notice do that while it is set, as it does for tools ([tools.md](tools.md)). Nothing is saved, and unsetting it does not step a prerelease down.
- `forest self update <version>` installs exactly that release, which is also the way back to stable from a prerelease. Install one exact release fresh with `... | bash -s -- <tag>`. Non-SemVer values such as `latest` are refused.
- Before replacing anything, every path checks that the `.sha256` file holds one record naming the archive and matching it, and that the new binary's `forest --version` is the requested version.

On v0.3.16 and older, plain `self update` installs GitHub's latest release even over a newer binary; `self update <tag>` already installs an exact prerelease once one is published.

`forest --version` does not reveal an unreleased build on v0.3.16 and older: it prints the last released number. After v0.3.16, release builds print `forest <version> (commit <sha>)` and builds from source `forest <version> (local build)`. When behaviour matters, check for the flag in `forest <cmd> --help` rather than trusting the version string.

## Contexts: which server a command talks to

A context is a named server plus its stored login, like a kubectl context.

```bash
forest context list            # active one marked *
forest context active          # name + server URL
forest context use <name>
forest context create <name> --server <url> [--use]
```

Resolution, highest first:

1. `--forest-server <url>` / `FOREST_SERVER` sets the **server**.
2. `--context <name>` / `FOREST_CONTEXT` picks the context.
3. The active context.

Credentials always come from the context (2 or 3), never from `FOREST_SERVER`. Pointing `FOREST_SERVER` at another server therefore sends the active context's login there. A `--context` naming a context that does not exist silently falls back to the active one; check `forest context list` for typos.

Mutating commands print the target, for example `→ Publishing org/name to <context> (<server>)`. Read that line, and check the context before running anything, not after.

## Logging in and tokens

```bash
forest auth login          # browser device-code flow on a TTY; --password for the legacy flow
forest auth status         # who you are (it does NOT print the server — use `forest context active`)
```

- **`FOREST_TOKEN`**, when set and non-empty, is sent as the bearer token and bypasses the stored login entirely. This is how CI authenticates. `forest auth status` still says "not logged in" with only a token, so test a token with a read such as `forest components list -o <org>`.
- **`forest auth token create --name <n> [--scopes a,b] [--expires-in <secs>]`** mints a personal access token carrying its creator's permissions. The raw token goes to stdout and everything else to stderr, so it can be piped into a secret store. Don't run it to "check" something, since it creates a credential.
- **OAuth apps** (`forest organisation oauth-app …`) give client-credentials tokens scoped to one organisation. The `components:publish` scope covers registry and publish calls only, so such a token cannot release. Some docs pages still say there is no machine token or describe a `forest organisation app` command; both are out of date.

## Projects and organisations

`forest.cue` names the project:

```cue
project: {
	name:         "my-service"   // ^[a-z][a-z0-9-]*$
	organisation: "my-org"       // ^[a-z][a-z0-9-]*$
}
```

Commands that take `-o/--organisation` and `-p/--project` read them from the `forest.cue` in the current directory when omitted. With no TTY, several list commands prompt and then fail with `The input device is not a TTY`, so pass `-o`/`-p` explicitly in scripts and agents. Scaffold a fresh checkout with `forest project init --project-name <name>`, which writes `forest.cue` and `cue.mod`.

Environments and destinations are organisation-level and shared by every project in the organisation:

```bash
forest environment list -o <org>
forest destination list -o <org>
forest destination types
```

## Where things live (Linux; macOS uses its platform dirs)

| Path | What |
|---|---|
| `~/.local/share/forest/contexts.json`, `…/forest/contexts/<name>/` | contexts and their stored logins |
| `~/.config/forest/forest.cue` | global tool selection (`forest global add` writes it) |
| `~/.local/state/forest/forest.lock` | global tools' locked digests |
| `~/.cache/forest/global/shims/` | tool shims to put on `PATH` |
| `~/.cache/forest/components/<org>/<name>/<version>/` | downloaded component files |
| `~/.cache/forest/components/bin/` | binaries, content-addressed by sha256 |
| `<project>/forest.lock` | the project's locked dependency digests |
| `<project>/.forest/` | build output and rendered deployment files; gitignore it |

All of these follow `XDG_*`. An isolated `HOME` plus `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME` and `TMPDIR` keeps an experiment away from the user's real setup. Add `FOREST_NO_AUTO_UPDATE=1 FOREST_NO_GLOBAL_WARM=1 FOREST_NO_UPDATE_CHECK=1` to stop background work, and leave `FOREST_PRERELEASE` unset unless the experiment is about prereleases (v0.3.16+: it makes unpinned tools follow prereleases, see [tools.md](tools.md); after v0.3.16 also forest's own `self update`, `self check` and update notice).

## Output for scripts

`--format json` on list/show commands gives typed JSON, and `--format name` gives the first column only. `-v`, `-vv` or `-vvv` (or `FOREST_LOG`) bring back structured logs when the rich UI hides what happened.
