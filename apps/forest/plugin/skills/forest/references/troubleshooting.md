# Diagnosing Forest failures

Work from the outside in, and change nothing until the cause is clear.

1. **Which CLI?** Run `forest --version`, and look for the flag in `forest <cmd> --help`. Since v0.3.17 a build from source prints `(local build)`; older builds print only the last released number.
2. **Which server and who?** `forest context active`, `forest auth status`, and whether overrides are set. In bash, `for name in FOREST_SERVER FOREST_CONTEXT FOREST_TOKEN; do if [[ -v $name ]]; then printf '%s is set\n' "$name"; fi; done` prints names only. Never dump `env`, tokens or stored logins into diagnostics.
3. **Which project?** Run from the directory holding `forest.cue`, or pass `-o`/`-p`. Is `forest.lock` committed and current?
4. **Get the real error.** Add `-v` or `-vv`; the rich UI can hide the cause. For releases use `forest release show <slug> -o <org> -p <project>`, and `--logs-only` for provider output.
5. **Match it** against the topic tables: [components.md](components.md), [publishing.md](publishing.md#failure-modes), [tools.md](tools.md#failure-modes), [releases.md](releases.md#failure-modes), [pipelines-and-policies.md](pipelines-and-policies.md).

## Cross-cutting symptoms

| Symptom | What it means | Next step |
|---|---|---|
| `failed to find forest.toml in project` (v0.3.16 and older) / `no forest.cue in this directory or any parent` | a project command (even `forest run --help`) outside a project directory | `cd` to the directory with `forest.cue` |
| `failed to find upstream component` | the dependency's `version:` is unpublished or not visible to this context, or (v0.3.16 and older) a range | write an exact version from `forest components show <org>/<name>`; check the context |
| `no published version of <org>/<name> matches '<spec>'` | since v0.3.17, a range nothing matches | widen the range or pin an exact version |
| `error: unrecognized subcommand '<comp>:<cmd>'` | the command isn't registered: no usage block, the method isn't named `commands/<cmd>`, or (v0.3.16 and older consumers) a version published by override whose `forest.cue` claims another version | `forest run --help` lists what *is* registered; see [publishing.md](publishing.md#component-prereleases) |
| the same `→ Downloading <org>/<name>@<v>` on every run | v0.3.16 and older: the cached copy's own `forest.cue` names a different version (the override case) | update the consumer's CLI, or publish a new version with the manifest carrying that version; do not unpublish to reuse a number |
| `WARN … skipping unparseable component in cache … components/include/<org>/<tool>` | v0.3.14 and older: global tools' include dirs share the component cache | harmless noise; v0.3.15 logs it at debug, and later CLIs skip those dirs |
| `The input device is not a TTY` | a picker or prompt with no terminal | pass `-o`, `-p`, the slug and the other required flags |
| panic in `forest docs` | v0.3.16 and older: `--format` collision, fixed in v0.3.17 | use `forest <cmd> --help` and the docs site |
| `unexpected argument '--prerelease'` | released CLI predates tool prereleases | see [tools.md](tools.md#tool-prereleases-release-status-matters) |
| `lock file mismatch` / `DigestChanged` / `sha mismatch — refusing to write to cache` | the bytes don't match the digest locked earlier | stop; ask the owner. Never re-pin by deleting lock lines |
| a new shell misbehaves after installing a tool | a tool's shell snippet | `FOREST_NO_SHELL_INTEGRATION=1`, then open a new shell to confirm |
| CI is green but the service didn't change | annotated only, the trigger didn't match (`--source-type`, unanchored or mismatched branch), a soak policy is holding, or a plan stage is awaiting approval | `release show <slug>`; `trigger list`; `policy evaluate` |
| released, but the old code still serves | the provider reported success on a rollback, or the wrong destination matched | check the running revision; compare `destination list` against the regexes |

## Reproducing safely

To test a theory without touching shared state, build or use the CLI against an isolated `HOME`/`XDG_*`/`TMPDIR` with `FOREST_NO_AUTO_UPDATE=1 FOREST_NO_GLOBAL_WARM=1 FOREST_NO_UPDATE_CHECK=1`, and a local forest-server: Postgres, NATS and MinIO via `apps/forest/templates/docker-compose.yaml` in the forest repo, plus `forest-server serve`. `forest/noop@1` destinations record releases and deploy nothing, which makes them good for rehearsing triggers, pipelines, policies and approvals. Never "try it" against a real context.
