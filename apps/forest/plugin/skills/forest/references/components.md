# Components: consuming, running, authoring

A component is a versioned package in the registry: a binary (or Deno entrypoint), its CUE contract and optional templates. Projects depend on components to get **commands** (run with `forest run`) and **hooks** (called by `forest release prepare` to render deployment files). A component with a `#Tool` facet is also a **tool**, installed with `forest global` (see [tools.md](tools.md)).

The authoritative schema is `apps/forest/components/forest/sdk/spec.cue` in the forest repo. `apps/forest/cue/forest-sdk/spec.cue` is an older copy that lags behind it.

## Consuming a component

```cue
dependencies: {
	"my-org/greeter":     version: "0.3.0"         // exact published version
	"my-org/in-progress": path: "../in-progress"   // local checkout, for development
}

"my-org": "greeter": {
	config: greeting: "hej"                       // checked against the component's #Spec
	env: prod: config: greeting: "hello"          // per-environment override
}
```

- **Prefer an exact, published `version:`.** It works on every CLI, and a prerelease such as `"1.1.0-rc.2"` works when written exactly. Since v0.3.17, `"0.3"`, `"0"` and `"latest"` resolve too: to a matching binary version lock in `forest.lock` when present, otherwise to the newest published stable match (a prerelease only if nothing stable matches). File/template-only components have no version lock; pin them exactly for reproducible selection. `"^0.3"` is not a supported spec. On **v0.3.16 and older**, any range fails every command that loads the project with `failed to find upstream component`. A range nothing matches fails with `no published version of <org>/<name> matches '<spec>'`.
- **A usage block is required.** A dependency without a `"<org>": "<name>": {…}` block contributes no commands.
- `forest add <org>/<name>` writes the highest **stable** version, plus a `cue.mod/module.cue` entry so the component's CUE types can be imported. `forest add <org>/<name>@<v>` writes `<v>` without checking that it exists; the next command then fails. `forest add` refuses a dependency that is already listed, so edit `forest.cue` to change a version.
- `forest add <org>/<name> --path <dir>` develops a component against a real consumer before publishing.
- Components called by other components are not resolved transitively. List every component the project's components call.

## Running commands: `forest run`

```bash
forest run --help                       # lists what this project exposes (and may download deps)
forest run test                         # project command from `commands:`
forest run greeter:hello --name kasper  # component command, qualified by component name
```

- A project's `commands: { test: ["cargo test"] }` runs each line with `bash -c "set -e; …"` in the project directory. Extra CLI arguments are **not** passed to it.
- A component command keeps its short name when that is unique. When a project command has the same name, the project wins the short name and the component's is reachable only as `<component>:<command>` (component name, no org).
- Arguments to a component command become its JSON input: `--key value`, `--flag` (true), `--key @file` and `--key @-` (stdin). Dashes in keys become underscores, and positional arguments are dropped.
- Every invocation resolves the project's dependencies first, so a broken dependency blocks even unrelated project commands.

`forest run` is project-scoped. To run an org tool anywhere, use its shim or `forest global run` (see [tools.md](tools.md)).

## Locking: `forest.lock` and `forest update`

```
# forest.lock — do not edit manually
my-org/greeter@0.3.0 linux/amd64 sha256:3d4714…
my-org/in-progress@0.1.0 path:../in-progress
```

- A binary dependency gets one line for the **host** platform. Its digest is checked on download. Since v0.3.17, the resolved cached binary's digest is also checked against the project's lock before commands are loaded, and a warm shared cache still creates a missing project pin. Released v0.3.16 checks the lock only on download. File/template-only dependencies have no version/digest lock.
- Lock-first range selection applies only where a registry **binary** lock entry exists. File/template-only components have no registry version/digest lock entry, so their ranges resolve again; use exact versions for reproducible selection. Do not infer binary digest protection for component files.
- An exact version pin controls selection, not lifetime content identity. Currently published versions reject overwrite, but admin unpublish permits reuse. Binary digest locks retain the earlier digest and reject replacement; warm cached binaries are not checked for online revocation. File/template-only versions have no binary digest protection.
- `forest update [<org>/<name>]` moves `forest.lock` to the newest version matching each spec. It **does not rewrite `forest.cue`**. With an exact version it just re-locks that version. On v0.3.16 and older it rejects dotted prereleases (`invalid version spec: '1.1.0-rc.2'`), and a range in `forest.cue` breaks every other command.
- After changing a binary version, run the consuming command to record its digest in `forest.lock`; use `forest update` to move a locked binary range forward. Inspect and commit the resulting `forest.cue` and `forest.lock`, and never delete a digest to bypass a mismatch.

## Validating

```bash
forest validate
```

It runs preflight checks on the project's own manifest, then calls `commands/validate` on each **path** dependency with the project's config. Registry dependencies are skipped. It exits 1 with each component's error list when a config is invalid. A typo in a version is not caught here; the first `forest run` or `forest release prepare` catches it.

## Authoring a component

A component repo has `forest.cue` (identity, how it is built and published) and `forest.component.cue` (its contract):

```cue
// forest.cue
project: {name: "greeter", organisation: "my-org"}

forest: component: {
	name:    project.name
	version: "0.3.0" // x-release-please-version
	upload: {
		source: "."
		type:   "go"   // rust | go | docker | typescript | deno | prebuilt
		architectures: linux: {amd64: {}, arm64: {}}
	}
}
```

```cue
// forest.component.cue
#Spec: {greeting: string | *"hello"}               // config consumers supply
#Commands: {hello: {description: "Greet", input: {name?: string}, output: {message: string}}}
#Hooks: {}                                          // e.g. "forest/deployment": prepare/release
#Tool: {name: "greeter", argv_passthrough: true}   // only if it is also a CLI tool
```

Real components import the SDK (`import sdk "forest.sh/forest/sdk@v0"`, then `sdk.#ForestComponent & {…}`). `#ForestInclude` is closed, so `include.shell` needs SDK `v0.8.0` or newer in `cue.mod/module.cue`.

These are **CUE module versions**, not project-dependency ranges: `forest.sh/forest/sdk@v0` selects the import's major namespace, while `cue.mod/module.cue` pins a module version such as `v0.8.0`. Do not replace valid SDK import syntax with a component's exact registry version. Tool selectors (`org/name@version`, catalogue subscriptions) are separate again; see [tools.md](tools.md).

**The binary protocol.** Forest probes `<binary> _meta/describe`, which prints JSON `{"protocol_version", "methods":[{"name","kind"}], "tool"?}`. Commands must be named `commands/<name>` in that list; a method named plain `hello` is never exposed. A call is `<binary> <method>` with `{"spec", "input", "context"}` on stdin, and the result JSON goes to stdout; non-zero exit means failure, with the message on stderr. The Rust SDK and the TypeScript generator (`forest generate --language typescript`) produce this for you.

**Building.** There is no `forest build`. `forest run build` dispatches the build component the project depends on (for example `forest-contrib/build-go`), which writes `.forest/component/output/<os>/<arch>/<name>`. `FOREST_BUILD_TARGETS=linux/amd64,…` narrows the declared matrix. `forest generate [--output <dir>] [--language rust|typescript]` regenerates bindings from the CUE spec; building does not regenerate them.

**Delivery shapes** (choose one):

- `upload.type: rust|go|docker|typescript|deno`: `publish` uploads every platform it finds staged under `.forest/component/output/`, and the host platform must be among them. Some older text, including a publish warning, still says only the host platform is uploaded.
- `upload.type: "prebuilt"` with `prebuilt: {linux: {amd64: "dist/linux/amd64/x"}, macos: {…}}`: for **tools** that you cross-build yourself. It requires `#Tool`, skips the describe probe and publishes no methods, so it cannot carry commands or hooks.
- `external: platforms: [{os, arch, url: "https://…", sha256, archive?, archive_sha256?, binary_in_archive?}]`: the binary stays upstream. `forest tool hash <url> --archive tar.gz --binary-in-archive <path>` prints both digests.

Optional on any shape: `include: {env: {K: "v"}, shell: init: {zsh: ["init","zsh"]}}`, which only lists shells the binary can actually emit, and `requires: tools: [{name: "cargo", hint: "…"}]`.

`publish` uploads the files under `source` alongside the binary, so keep scratch files out of the component directory or list globs in `forest: component: paths: exclude: [...]`.

Next: [publishing.md](publishing.md).
