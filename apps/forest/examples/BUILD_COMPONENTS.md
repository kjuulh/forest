# Build components (DATA-312) — examples & smoke test

`forest build` is no longer a CLI command. Building is now a **depended-on
component**: a project declares a dependency on one of `forest-contrib/build-rust`,
`build-go`, or `build-docker`, and `forest run build` dispatches to it. The
component reads the project's manifest, shells out to its toolchain (cargo / go
/ docker), and writes the artifact — all behaviour lives in the component, not
in a hard-coded CLI branch.

The three examples here each depend on the matching build component:

| Example | Depends on | Toolchain |
|---|---|---|
| `build-rust-example/`   | `forest-contrib/build-rust`   | `cargo` |
| `build-go-example/`     | `forest-contrib/build-go`     | `go` |
| `build-docker-example/` | `forest-contrib/build-docker` | `docker buildx` |

Each declares the dependency as a local `path:` dep and a usage block, which is
what surfaces the component's `commands/build` as `forest run build`:

```cue
dependencies: {
    "forest-contrib/build-rust": path: "../../components/forest-contrib/build-rust"
}
"forest-contrib": "build-rust": {}
```

## Offline smoke (no registry needed)

The build path is covered by an offline integration test that drives the real
`run_build` (manifest read → target resolution → `cargo` → summary) against a
self-contained manifest:

```sh
cargo test -p forest-build-core --test run_build_smoke
```

It skips gracefully if `cargo +nightly` / `cue` aren't present.

## End-to-end against a registry

Running the examples needs a CUE registry that serves the current `forest/sdk`
(for the `forest.sh/forest/sdk@v0` import). The build component itself is a
local `path:` dependency, so it does **not** need to be published for
`forest run build`.

1. Point `CUE_REGISTRY` at a registry serving `forest/sdk`, e.g. the dev
   registry, or a local server (`forest-server serve` → OCI on `:4042`, use
   `forest.sh=localhost:4042+insecure`).
2. Build the build-component binaries once (bootstrap, since `forest build` is
   gone): `cargo build -p build-rust -p build-go -p build-docker`.
3. Run the build through the new path:

   ```sh
   cd examples/build-rust-example
   forest run build        # dispatches to forest-contrib/build-rust
   ```

   You'll see the tool gate (it fails up front with a miette diagnostic if
   `cargo`/`cue` is missing), live cargo output (passthrough mode), and a JSON
   summary of built artifacts.
4. `forest publish` then uploads the artifact to the registry (needs an
   authenticated context / org).

> Note: the public dev registry may serve an older `forest/sdk` that rejects
> `project.description` / `metadata`. Publish the current sdk to your own
> registry, or omit those optional fields, if you hit
> `field not allowed`.

### Validated

`build-rust-example` was validated end-to-end on macOS/arm64: `forest run build`
resolved the `build-rust` path dependency, passed the `cargo`+`cue` tool gate,
invoked the component in passthrough mode, compiled the crate, and returned the
artifact summary. The missing-tool diagnostic was confirmed by running with
`cargo` off `PATH`.
