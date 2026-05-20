# forest/sdk

The base CUE module every Forest component imports. Defines the shapes
that the rest of the ecosystem speaks: `#ForestProject`,
`#ForestComponent`, command and hook schemas, source/architecture
enums, and the (now optional) project `description` + `metadata` block
introduced in spec 009.

## What lives here

- **`#ForestProject`** — project identity (`name`, `organisation`),
  plus the spec-009 surface (`description`, `metadata`).
- **`#ForestComponent`** — component identity (`name`, `version`),
  build descriptor (`codegen`, `upload`), optional `external` manifest.
- **`#ForestCommands` / `#ForestCommand`** — the typed command surface
  exposed via `forest run`.
- **`#ForestHooks` / `#ForestHook`** — pre/post-deploy hooks.
- **`#ForestTool`** — the global-tool facet (TASKS/018) carried on
  components that ship as CLI binaries.

## Using it

```cue
import sdk "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
    name:         "my-thing"
    organisation: "my-org"
}

forest: component: sdk.#ForestComponent & {
    name:    project.name
    version: "0.1.0"
    upload: { /* … */ }
}
```

Bumping the SDK is a coordinated effort: every published component
imports an exact major (`@v0`) and is validated against the schema at
publish time.
