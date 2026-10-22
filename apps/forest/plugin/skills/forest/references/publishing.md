# Publishing components and component prereleases

Publishing uploads a component version (binary or binaries, CUE files, manifest) to the registry of the active context. It is not a deployment: nothing runs anywhere until a project depends on that version, or a user installs it as a tool, and then releases or runs it. Publishing a container image is a separate step again: CI pushes images, and a release refers to them, typically with `--set`.

## Prerequisites

- The right context and organisation. The organisation comes from `project.organisation` and the name from `forest.component.name`; the server needs org membership and publish rights.
- The build already ran, for built types. For prebuilt, the per-platform files exist.
- Use a fresh version number. The registry refuses overwrite while that version is published, prereleases included; unpublish allows reuse, so a version number is not a lifetime content identity.

## Shortest successful path

```bash
forest context active                 # right server?
forest run build                      # built types only
forest publish --dry-run              # preflight
forest publish
forest components show <org>/<name>   # the version is listed: this is the proof
```

`--dry-run` does not contact the registry, but it is **not** free of side effects or requirements:

- It needs a login (or `FOREST_TOKEN`).
- It runs `cue export`, plus `cue eval -e #Tool` for prebuilt and external shapes.
- For a built binary, preflight can execute `_meta/describe`, copy the binary into the local cache and rewrite `.forest/component/meta.json`. A cached descriptor can be reused, so this is not necessarily a fresh probe.
- Prebuilt and external shapes skip `_meta/describe` and do not copy a binary into the cache or write `.forest/component/meta.json`; their publish manifest is built in memory.
- It prints the target context (`→ Publishing org/name to <ctx> (<server>)`). Through v0.3.16 a **prebuilt** dry-run omits that line, so check `forest context active` yourself.

A good dry-run ends with `dry-run: would publish org/name@ver as shape=<component|tool_binary> [binary] <platforms>`. `[files]` instead of `[binary]` means no binary was found, and the publish would ship CUE only.

## Versions

Precedence: `--version` > `FOREST_COMPONENT_VERSION` > `forest.component.version` in `forest.cue`. Since v0.3.15, `forest components version` prints the result, and a prebuilt build script should stamp its binary with it. Forest's own rust/go/docker build components read the env var themselves.

- For commands/hooks consumed as project dependencies, set `forest.component.version` in `forest.cue` **before building**, so manifest, binary and registry agree. For tools, an override is supported: prefer `FOREST_COMPONENT_VERSION` during both build and publish over a publish-only `--version` flag, which can leave a stale binary stamp.
- The registry orders versions by SemVer: `1.3.0-alpha.10` > `1.3.0-alpha.2`, `1.3.0` > `1.3.0-rc.2`, and `rc.10` follows `rc.9`.
- Republishing a currently published version fails cleanly (`version X is already published`), so re-running a CI tag does not overwrite it. An interrupted upload is aborted, which frees the version for a retry.
- There is no yank. An org admin's hidden `forest admin unpublish` frees a version number for reuse. Existing binary digest locks retain the first digest and reject changed bytes when resolved or fetched; a warm cached binary may continue running without an online revocation check. File/template-only components have no binary digest lock. Ship a new number instead of reusing one.

## Component prereleases

A prerelease is just a SemVer version with a pre-release suffix: `0.4.0-rc.1`, `0.4.0-alpha.2`. What reaches whom:

| Consumer | Default (no version given) | Exact version | Range |
|---|---|---|---|
| Project dependency (`forest.cue`) | n/a: a version is required | `version: "0.4.0-rc.1"` works | since v0.3.17, resolves to a matching binary lock entry or newest **stable** match (an RC only if nothing stable matches); file/template-only components have no version lock; v0.3.16 and older fail at runtime |
| `forest add <org>/<name>` | highest **stable** | `@0.4.0-rc.1` written verbatim | n/a |
| Tool (`forest global add`) | highest stable; v0.3.15+ refuses an RC-only tool unless `--prerelease` ([tools.md](tools.md)) | `@0.4.0-rc.1` pins it | not supported |
| Org tool catalogue | stable only | n/a | n/a |

Projects and `forest add` need an exact RC selection. Tool defaults differ: v0.3.14 can silently select an RC-only tool, while v0.3.15 offers explicit prerelease tracking; see [tools.md](tools.md#tool-prereleases-release-status-matters).

## Publish, consume and run a component RC

Set the producer's `forest.component.version` to `0.4.0-rc.1` in `forest.cue`, then run its existing build:

```bash
forest context active
forest run build
forest publish --dry-run
forest publish
forest components show my-org/greeter
```

Confirm that exact RC appears in the registry. In a separate consumer's `forest.cue`:

```cue
dependencies: "my-org/greeter": version: "0.4.0-rc.1"
"my-org": greeter: config: greeting: "hej"
```

Run a real command that this component exports:

```bash
forest run greeter:hello --name kasper
```

Check its RC behavior, then run again to confirm reuse, and inspect the RC's `sha256:` entry in `forest.lock`. On v0.3.16 and older, don't run `forest update` here: it rejects dotted RCs (`invalid version spec`). Since v0.3.17 it accepts them. Commit the consumer manifest and generated lock. Registry listing proves publication; actual output proves consumption/execution.

Publishing an RC only through `FOREST_COMPONENT_VERSION`/`--version` while `forest.cue` still says `0.3.0` uploads a `forest.cue` that claims `0.3.0`. A **consumer on v0.3.16 or older** that depends on `0.4.0-rc.1` re-downloads it on every command and never registers its commands; for example, `forest run greeter:hello` fails as an unknown subcommand. Consumers on v0.3.17 and newer identify the cached component by its registry version, so they work. Because you can't know every consumer's CLI version, commit the matching manifest before publishing a component that others consume. Tools are unaffected either way, since they are resolved from the manifest.

Return to stable by pinning its exact published version and running the consumer command again. Use a new number for each rebuilt RC, even if the previous one was broken; unpublishing and reusing its number does not update existing binary digest locks.

## Publishing from CI

Inside Understory, use the reusable pieces in `understory-io/forest` rather than hand-rolled steps:

- `.github/workflows/forest-publish.yml` on `push: tags: ["v*"]`: resolves the version from the tag, builds a matrix, publishes once, then reads the registry back. For a component consumed by commands/hooks, commit the matching prerelease manifest **before** tagging `v0.4.0-rc.1`; the workflow's `FOREST_COMPONENT_VERSION` override alone does not repair the uploaded manifest. Tag/build/upload consistency is a prerequisite, not optional troubleshooting.
- `.github/actions/setup-forest` plus `.github/actions/publish`: for a repo that already builds its own binaries.
- Secrets: `FOREST_TOKEN` (a token with publish rights in the org), and the org-wide `GO_PRIVATE_MODULES_PAT` passed as `forest-repo-token`, because `install.sh` downloads the CLI from a private repo.
- Keep `forest.component.version` annotated `// x-release-please-version` so release-please bumps it with the tag. The workflow fails when a stable tag disagrees with the manifest.

The current inputs are in that repo's `apps/forest/docs/docs/guides/ci-cd.md`. Read it rather than copying versions from here.

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `version X is already published` | overwrite of a currently published version is refused | bump the number; do not unpublish to reuse it |
| `cue eval -e #Tool failed: reference "#Tool" not found` | prebuilt or external shape without `#Tool` | add `#Tool`, or use a built type for a component with commands |
| `cue export rejected the manifest … expected ','` | invalid CUE, often a hyphen in `package` | fix the reported field; package names use `_` |
| `does not implement the _meta/describe subcommand` | the binary doesn't speak the protocol | implement it (the SDK does), or publish as prebuilt/external with `#Tool` |
| `! publishing a tool for linux_amd64 only` | one platform staged | build or add the other platforms; users elsewhere get `not available for <os>/<arch>` |
| `Version overridden: X (forest.cue) → Y` | an override is active | expected in CI; for consumed components see the prerelease caveat |
| not logged in / unauthenticated | no login and no `FOREST_TOKEN` | `forest auth login`, or set the token in CI |
