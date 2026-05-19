# 011: `forest build` at project level should cascade codegen to components

## Problem

When you change a component's `forest.component.cue` spec (e.g. add a new field), you must manually:

1. `cd components/my-component && forest generate --output ./src/`
2. `cd components/my-component && forest build`
3. `cd deployment && forest release prepare`

Just running `forest release prepare` (or `forest release create`) from the project doesn't regenerate the component's codegen. You get stale `forestgen.ts` with the old spec until you manually regenerate.

This is especially confusing because `forest build` at the component level does work — it just doesn't trigger codegen.

## Desired behavior

When `forest release prepare` (or a new `forest build` at project level) runs:

1. For each component dependency with a local path, check if `forest.component.cue` is newer than `forestgen.ts`
2. If so, re-run codegen automatically
3. Then run the component build (meta.json generation)
4. Then proceed with template rendering and hook invocation

## Implementation plan

### Option A: Automatic codegen in prepare

In `crates/forest/src/cli/release/prepare.rs`, before invoking component hooks:
1. Resolve each component dependency path
2. Check mtime of `forest.component.cue` vs `src/forestgen.ts`
3. If spec is newer, run codegen

### Option B: `forest build --all` at project level

Add a project-level build command that:
1. Iterates over all component dependencies
2. Runs `forest generate` + `forest build` for each
3. Can be used as a pre-step before `forest release prepare`

### Option C: Make `forest build` at component level run codegen

Change `forest build` to always run codegen before generating `meta.json`. This is the simplest fix — `build` becomes the single command that does everything.

## Files to change

- `crates/forest/src/cli/components/build.rs` — optionally run codegen before meta.json generation
- OR `crates/forest/src/cli/release/prepare.rs` — add codegen step before hook invocation
