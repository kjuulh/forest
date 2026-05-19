# forest-hello

A minimal example of a Forest **TOOL_BINARY** — a global tool that ships as
a single binary you install once and run anywhere via the forest shim PATH.

## What it does

Prints `hello, <arg>!` for whatever argument you pass on the command line:

```sh
$ forest-hello world
hello, world!
```

It's an argv-passthrough binary: Forest forwards every CLI argument to the
underlying executable verbatim. There's no `_meta/describe` protocol, no
methods to dispatch — the binary just runs.

## Install

```sh
forest global add <org>/forest-hello
```

After install, you can invoke it three ways:

```sh
forest global run <org>/forest-hello -- world         # qualified
forest global run forest-hello world                  # bare name (via shim)
forest-hello world                                     # PATH (via eval)
```

To get the shim on your PATH, source the eval script in your shell rc:

```sh
eval "$(forest eval bash)"      # or `zsh`
```

## Shape

This component declares only a `#Tool` facet (no `#Commands`), which makes
the registry classify it as **TOOL_BINARY**:

- `kind: binary` — Forest hosts the binary content-addressed in the registry
- `tool: { name, argv_passthrough: true, description }` — global install +
  argv passthrough
- no `methods[]` — no describe protocol

See `forest.component.cue` for the full declaration.

## Spec links

- The shape taxonomy lives in `TASKS/018-global-tools.md §1a.2e`.
- The example sits inside `examples/global-tools/` next to its siblings
  `forest-greet` (HYBRID — methods + tool), `forest-ripgrep` (TOOL_EXTERNAL
  — upstream URL), and friends.

## Updating this README

Run `forest publish` from this directory — Forest auto-uploads `README.md`
alongside the binary. The README shows up on the project's Overview page
in Forage.
