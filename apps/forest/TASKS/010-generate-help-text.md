# 010: Update `forest generate --help` to describe all output types

## Problem

The `--help` text says:

```
Generate Rust SDK code from CUE component spec (forest.component.cue)
```

This is misleading because:
1. It generates TypeScript, not Rust (when `--language typescript` or auto-detected from `forest.cue`)
2. It also generates `deps/*.ts` dependency client files, which isn't mentioned at all
3. The description says "Rust SDK" even though TypeScript is the primary codegen target

## Fix

Update the help text in `crates/forest/src/cli/components/generate.rs`:

```
Generate type-safe code from the CUE component spec (forest.component.cue).

Generates:
  - forestgen.ts / forestgen.rs — Component interfaces, router, and type definitions
  - deps/<org>_<name>.ts — Client stubs for each declared dependency

Language is auto-detected from forest.cue codegen config, or set with --language.
```

## Files to change

- `crates/forest/src/cli/components/generate.rs` — update command description and about text
