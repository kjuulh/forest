# Component Exchange quickstart

This walkthrough exercises Forest's core product loop:

```text
discover → add and lock → typed run → install tool facet
```

It requires access to a Forest server where your organisation has published a
component with both a command and a tool facet. Replace the `acme/*` examples
with coordinates supplied by your operator. A fresh local server is not seeded
with these example components automatically.

## Prerequisites

- the `forest` CLI from the [installation guide](getting-started/installation.md);
- CUE available on `PATH`;
- a server URL and an invited or registered account;
- a component coordinate and version from a trusted publisher.

## 1. Select the server

Create a named context so credentials and endpoints do not leak between
development, staging, and production:

```bash
forest context create team \
  --server https://api.forest.example.com \
  --use
forest context active
```

## 2. Authenticate

```bash
forest auth login --web
forest auth status
```

Use `forest auth login --password` only when the server and workflow require the
legacy password flow. CI should use a scoped, expiring machine identity when
that capability ships; during the private preview it can use a dedicated,
least-privileged personal token as documented in
[CI/CD integration](guides/ci-cd.md).

## 3. Discover a component

```bash
forest components list --org acme
forest components show acme/devctl
```

Inspect the publisher, version, available platforms, component shape, methods,
and checksums before installing. During the private preview, only run components
from organisations you trust.

## 4. Add and lock it in a project

From a directory containing `forest.cue`:

```bash
forest add acme/devctl@2.1.0
```

`forest add` records the dependency. Declare its usage block in `forest.cue` so
its commands enter the project command graph:

```cue
"acme": "devctl": {}
```

```bash
forest validate
forest run status
```

Forest currently executes exact registry versions reliably; use an exact
version rather than a range. Commit `forest.cue` and `forest.lock`. A clean
checkout should resolve the same artifact hashes.

The concrete commands and inputs come from the component's CUE contract. Use
`forest components show acme/devctl` to inspect them; replace `status` above if
the component exposes a different command.


## 5. Install its developer tool

If the component exposes a tool facet:

```bash
forest global add acme/devctl@2.1.0
forest global list
forest global which devctl
```

Enable shims and declared shell integration:

```bash
eval "$(forest shell zsh)"       # bash: forest shell bash
devctl --version
```

The shim fetches the platform artifact lazily, verifies its recorded checksum,
and caches it. Shell setup may also execute trusted tools during background
warming to capture output that later shells source. This verifies artifact
identity; it does not sandbox the binary or its shell code. See
[Security and sandboxing](product/security-and-sandboxing.md).

## 6. Verify reproducibility

In a second clean checkout:

```bash
forest validate
forest run status
```

Success means the checked-in exact version and recorded hashes resolve without
manual component setup. If Forest rewrites the lock unexpectedly, downloads a
different checksum, or needs an author's untracked files, the workflow is not
reproducible.

## Publish your own

Once the consumer path works, follow
[Authoring components](guides/authoring-components.md). The safe publication
sequence is:

```bash
forest add forest-contrib/build-rust@0.1.2
# Add `"forest-contrib": "build-rust": {}` to forest.cue.
forest generate
forest run build
forest publish --dry-run
forest publish
```

`forest build` is not a command. Builds are provided by a component and invoked
through `forest run build`.
