# Example — Forest Global Tools (the "enterprise mise" demo)

This example walks through the full UX of the forest global-tools system as specified in [`TASKS/018-global-tools.md`](../../TASKS/018-global-tools.md). It is **Phase-2 artefact #1** under VSDD: the example is built before the tests, the tests before the implementation. Every command in this README is part of the contract the implementation must satisfy.

If you're reading this before the feature has landed, treat it as a fiction-that-becomes-true: every `$ forest …` line must produce the stated output once the spec is implemented.

---

## What this example demonstrates

The shape taxonomy from [§1a.2e](../../TASKS/018-global-tools.md), with **every artefact authored as a normal Forest project** — a directory containing `forest.cue` + `forest.component.cue`, exactly like a regular component. The only thing that changes is what goes inside those files.

| Forest project | Shape | What's special in `forest.cue` |
|---|---|---|
| `forest-hello/` | `TOOL_BINARY` | `upload:` (Rust crate, hosted in Forest registry); `forest.component.cue` declares `#Tool` only |
| `forest-greet/` | `HYBRID_COMPONENT` | `upload:` + `forest.component.cue` declares BOTH `#Tool` and `#Commands` |
| `forest-ripgrep/` | `TOOL_EXTERNAL` | `external:` (no `upload`, no `codegen`); `forest.component.cue` declares `#Tool` only |
| `forest-jq/` | `TOOL_EXTERNAL` | `external:` with `archive: "none"` (bare executable) |
| `forest-fd/` | `TOOL_EXTERNAL` | `external:` with nested binary in tar.gz |

There is **one publishing command** for everything: `forest components publish`, run from inside the project directory. It dispatches on whether `forest.cue` carries `upload:` (build + UploadBinary path) or `external:` (manifest-only path). No separate `forest tool publish` subcommand.

Two valid layouts for external tools:

- **One project per tool** (used in this example) — `forest-ripgrep/`, `forest-jq/`, `forest-fd/` each have their own `forest.cue`. Mirrors how components are organised today.
- **Monorepo** — a single `forest-external-tools/` project that exposes many tools in one place. The spec is agnostic; pick what fits your org's release cadence.

The walkthrough covers:

1. **Publishing** all artefact shapes via the unified `forest components publish`.
2. **Subscribing** as a user — per-tool (`forest global add cuteorg/ripgrep`) and whole-catalogue (`forest global add cuteorg`).
3. **Banning** a specific tool from a catalogue subscription.
4. **Shell integration** via `forest eval zsh`.
5. **Lazy invocation** — first call downloads, verifies sha, caches, exec's.
6. **Cache inspection** — `forest global list`, `forest global which`, `forest global verify`.
7. **Reproducible installs** via `forest.lock`.
8. **Symmetry with `forest run`** — same name resolution in both surfaces.

---

## Prerequisites

- A Forest server running locally (`forest serve` in another terminal) or pointed at by `FOREST_SERVER_URL`.
- `forest auth login` completed against a test organisation called `cuteorg` (admin role required for publishing).
- `cue` on `$PATH` (per Q6.a). Future work: Forest auto-installs `cue` via FU1.
- Linux/amd64 or macOS/arm64 for the demo (other platforms work but the example only ships those two).

```sh
$ forest auth status
logged in as: alice@example.com
organisation: cuteorg (admin)
```

---

## Part 1 — Publish a `TOOL_BINARY` (in-registry hosted tool)

`forest-hello/` is a normal Forest project containing a 50-line Rust binary. Its `forest.cue` has `upload:` (it builds + ships its own binary). Its `forest.component.cue` declares `#Tool` and nothing else, which makes the registry shape `TOOL_BINARY`.

```sh
$ cd forest-hello

$ forest components build
building forest-hello@0.1.0 for linux/amd64 ...
  → target/release/forest-hello (sha256: 4f9c3a…  1.2 MiB)
building forest-hello@0.1.0 for darwin/arm64 ...
  → target/release-darwin/forest-hello (sha256: 7e21b8…  1.3 MiB)
manifest written: target/forest.component.manifest.json (kind=binary, shape=tool_binary)

$ forest components publish
uploading binary linux/amd64 (sha256:4f9c3a…) … done
uploading binary darwin/arm64 (sha256:7e21b8…) … done
publishing manifest … done
  cuteorg/forest-hello@0.1.0 shape=tool_binary
  tool.name=hello argv_passthrough=true
```

Inspecting the registry:

```sh
$ forest components show cuteorg/forest-hello
cuteorg/forest-hello @ 0.1.0
  shape:     tool_binary
  tool:      hello (argv_passthrough)
  platforms: linux/amd64 (1.2 MiB), darwin/arm64 (1.3 MiB)
  hosted:    Forest registry
```

---

## Part 2 — Publish a `HYBRID_COMPONENT`

`forest-greet/` is the same Rust binary as `forest-hello`, but its `forest.component.cue` declares BOTH `#Commands.greet` and a `#Tool` facet. Same publishing flow:

```sh
$ cd ../forest-greet
$ forest components build && forest components publish
…
  cuteorg/forest-greet@0.1.0 shape=hybrid_component
  tool.name=greet  methods=[greet]

$ forest components show cuteorg/forest-greet
cuteorg/forest-greet @ 0.1.0
  shape:     hybrid_component
  tool:      greet (argv_passthrough)
  methods:   greet
  platforms: linux/amd64, darwin/arm64
  hosted:    Forest registry
```

---

## Part 3 — Publish three `TOOL_EXTERNAL` projects

External tools are also Forest projects. Each has its own `forest.cue` declaring an `external:` block (instead of `upload:`); `forest components publish` dispatches on which block is present.

First, compute the sha256s for ripgrep (do this once per tool/version):

```sh
$ forest tool hash https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz \
                  --archive tar.gz --binary-in-archive ripgrep-14.1.1-x86_64-unknown-linux-musl/rg
archive_sha256: 4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef09337b3995f5f
binary_sha256:  ad3a44e3d8b8a9d39c1f7b4d1a9b9e3a5e7c2f6c8b4f3a1d2e9c8b7a6e5d4c3b
```

Paste the values into `forest-ripgrep/forest.cue` (the file ships with placeholders pointing at the right URL), then publish — same command as Parts 1 and 2:

```sh
$ cd ../forest-ripgrep
$ forest components publish
detected external manifest (no `upload:` block; `external:` declared)
validating CUE against #ToolManifest … ok
publishing cuteorg/ripgrep@14.1.1 (kind=external, shape=tool_external) … done
  upstream_host: github.com (full URL stored, surfaced only on detail)
  platforms:     linux/amd64, macos/arm64

$ cd ../forest-jq && forest components publish
publishing cuteorg/jq@1.7.1 (kind=external) … done

$ cd ../forest-fd && forest components publish
publishing cuteorg/fd@10.2.0 (kind=external) … done

$ forest components search rg --org cuteorg
cuteorg/ripgrep      @ 14.1.1   [tool-ext] ← github.com
cuteorg/forest-greet @ 0.1.0    [hybrid]
cuteorg/forest-hello @ 0.1.0    [tool]
cuteorg/fd           @ 10.2.0   [tool-ext] ← github.com
cuteorg/jq           @ 1.7.1    [tool-ext] ← github.com
```

Notice the shape badges. `forest global list` (Part 6) will filter to the tool-y subset.

---

## Part 4 — User-side: install + shell integration

Switch to a fresh user account on a different machine (or just blow away `~/.config/forest/`):

```sh
$ forest auth login
$ ls ~/.config/forest/
# (empty)
```

### 4a. Single tool, explicit pin

```sh
$ forest global add cuteorg/ripgrep
resolved cuteorg/ripgrep → 14.1.1
wrote ~/.config/forest/forest.cue
shim created: ~/.cache/forest/global/shims/rg
binary NOT downloaded (lazy — fetched on first invocation)
```

Inspect the new state:

```sh
$ cat ~/.config/forest/forest.cue
package forest

import sdk "forest.sh/forest/sdk@v0"

config: sdk.#UserConfig & {
    dependencies: {
        "cuteorg/ripgrep": { version: "14.1.1" }
    }
}

$ cat ~/.cache/forest/global/shims/rg
#!/bin/sh
# forest shim — do not edit
exec forest global run cuteorg/ripgrep -- "$@"

$ ls ~/.local/state/forest/
# (still empty — lockfile is written on first invocation)
```

### 4b. Subscribe to the whole org catalogue

```sh
$ forest global add cuteorg --ban forest-greet
resolved org catalogue 'cuteorg' (4 tools)
  + hello       (cuteorg/forest-hello@0.1.0)    [tool]
  + jq          (cuteorg/jq@1.7.1)              [tool-ext]
  + fd          (cuteorg/fd@10.2.0)             [tool-ext]
  − greet       BANNED
already pinned:
  rg            (cuteorg/ripgrep@14.1.1)        [tool-ext] (per-tool pin wins)
wrote ~/.config/forest/forest.cue
3 shims created in ~/.cache/forest/global/shims/
```

```sh
$ cat ~/.config/forest/forest.cue
package forest

import sdk "forest.sh/forest/sdk@v0"

config: sdk.#UserConfig & {
    dependencies: {
        "cuteorg/ripgrep": { version: "14.1.1" }
    }

    org_catalog: {
        cuteorg: {
            enabled: true
            banned:  ["forest-greet"]
        }
    }
}

$ ls ~/.cache/forest/global/shims/
fd  hello  jq  rg
```

### 4c. Shell integration

```sh
$ echo 'eval "$(forest eval zsh)"' >> ~/.zshrc
$ source ~/.zshrc

$ echo $PATH | tr ':' '\n' | head -3
/home/alice/.cache/forest/global/shims
/usr/local/bin
/usr/bin
```

`forest eval zsh` is idempotent — sourcing `.zshrc` twice does not duplicate the PATH entry.

---

## Part 5 — Lazy invocation

First call to `rg`:

```sh
$ rg --version
forest: cold cache for cuteorg/ripgrep@14.1.1 (linux/amd64)
forest: fetching https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/...
forest: archive_sha256 verified (4cf9f274…)
forest: extracted ripgrep-14.1.1-x86_64-unknown-linux-musl/rg
forest: binary_sha256 verified (ad3a44e3…)
forest: cached at ~/.cache/forest/components/bin/ad3a44e3…
forest: lockfile updated
ripgrep 14.1.1
features:+pcre2 …
```

Second call (warm):

```sh
$ time rg foo /tmp/test.txt
(no output, exit 1)
real    0m0.031s     # ≤ 60 ms per NFR §1a.11
user    0m0.018s
sys     0m0.011s
```

Lockfile now has the pin:

```sh
$ cat ~/.local/state/forest/forest.lock
# forest.lock — do not edit manually
cuteorg/ripgrep@14.1.1 linux/amd64 sha256:ad3a44e3…
```

---

## Part 6 — Inspecting installed tools

```sh
$ forest global list
NAME    SOURCE                          VERSION    SHAPE       STATUS
fd      cuteorg/fd (via org-catalog)    10.2.0     tool-ext    missing
hello   cuteorg/forest-hello (org-cat)  0.1.0      tool        missing
jq      cuteorg/jq (via org-catalog)    1.7.1      tool-ext    missing
rg      cuteorg/ripgrep                 14.1.1     tool-ext    cached
shadowed (per-tool pin overrides catalogue):
   rg via [dependencies] → catalogue entry ignored

$ forest global which rg
~/.cache/forest/components/bin/ad3a44e3…

$ forest global which fd
forest: cold cache for cuteorg/fd@10.2.0
forest: fetching ...
~/.cache/forest/components/bin/9b2c1e7f…
```

---

## Part 7 — Defence-in-depth re-verification

```sh
$ forest global verify
scanning ~/.cache/forest/components/bin/
  ad3a44e3…  ok
  9b2c1e7f…  ok
2 entries, 0 mismatches

$ echo bogus >> ~/.cache/forest/components/bin/ad3a44e3…
$ forest global verify
scanning ~/.cache/forest/components/bin/
  ad3a44e3…  MISMATCH (computed: 11e2…) → deleted
  9b2c1e7f…  ok
2 entries, 1 mismatch (deleted)
$ rg --version
forest: cold cache (recovering from deleted entry)
forest: fetching ...
ripgrep 14.1.1
```

The warm path trusts the cache (Q9.a / T1); `verify` is the explicit re-hash knob.

---

## Part 8 — Symmetry with `forest run`

`forest-greet` is a hybrid: its `greet` method works inside a Forest project, the shim works anywhere.

Inside a project (component protocol):

```sh
$ cd ~/projects/some-project
$ cat forest.cue
# (declares cuteorg/forest-greet as a project dep)

$ forest run greet --name=world
{"greeting":"hello, world!"}
```

Anywhere on PATH (argv passthrough via shim):

```sh
$ greet world
hello, world!
```

Bare-name resolution on `forest global run`:

```sh
$ forest global run hello
hello, anonymous!

$ forest global run rg --version
ripgrep 14.1.1
```

If two tools shared the same shim name (prevented at `add` time per §1a.8):

```sh
$ forest global run scaffolder
error: ambiguous tool name 'scaffolder'; specify <org>/<name>[@<version>]
```

Same UX as `forest run <command>` for ambiguous component commands.

---

## Part 9 — Ban / unban / update / removal

```sh
$ forest global ban cuteorg jq
deleted shim: ~/.cache/forest/global/shims/jq
banned cuteorg/jq from catalogue subscription

$ jq --version
zsh: command not found: jq

$ forest global unban cuteorg jq
created shim: ~/.cache/forest/global/shims/jq

$ forest global update
re-resolving 1 per-tool pin and 1 catalogue subscription...
  cuteorg/ripgrep         14.1.1 → 14.1.1 (no change)
  cuteorg catalogue:
    + cuteorg/forest-hello 0.1.0 → 0.1.0
    + cuteorg/jq           1.7.1 → 1.7.2 (NEW)
    + cuteorg/fd           10.2.0 → 10.2.0
shims synchronised

$ forest global remove cuteorg
unsubscribed from cuteorg catalogue
deleted shims: fd, hello, jq
kept (per-tool pin): rg
```

---

## Files in this directory

```
examples/global-tools/
├── README.md                          # this file
├── forest-hello/                      # TOOL_BINARY — own Rust binary
│   ├── forest.cue                       upload: rust
│   ├── forest.component.cue             #Tool only
│   ├── cue.mod/module.cue
│   └── crates/forest-hello/{Cargo.toml,src/main.rs}
├── forest-greet/                      # HYBRID_COMPONENT — own Rust binary
│   ├── forest.cue                       upload: rust
│   ├── forest.component.cue             #Tool + #Commands
│   ├── cue.mod/module.cue
│   └── crates/forest-greet/{Cargo.toml,src/main.rs}
├── forest-ripgrep/                    # TOOL_EXTERNAL — upstream URLs, tar.gz
│   ├── forest.cue                       external: { platforms: [...] }
│   ├── forest.component.cue             #Tool only
│   └── cue.mod/module.cue
├── forest-jq/                         # TOOL_EXTERNAL — bare executable
│   ├── forest.cue
│   ├── forest.component.cue
│   └── cue.mod/module.cue
├── forest-fd/                         # TOOL_EXTERNAL — tar.gz
│   ├── forest.cue
│   ├── forest.component.cue
│   └── cue.mod/module.cue
└── user-config/
    └── forest.cue                     # sample ~/.config/forest/forest.cue
```
