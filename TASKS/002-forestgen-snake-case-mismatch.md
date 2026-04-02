# 002: Fix forestgen.ts camelCase properties not matching runtime JSON

## Problem

The generated `forestgen.ts` converts snake_case CUE fields to camelCase TypeScript properties:

```typescript
// Generated:
export interface Spec {
  /** JSON field: "forage_postgresql" */
  foragePostgresql?: Record<string, unknown>;
}
```

But the Forest runtime passes raw JSON to the component without key transformation. At runtime, `spec.foragePostgresql` is `undefined` — the data is at `spec["forage_postgresql"]`.

The JSDoc `/** JSON field: ... */` annotation is documentation-only; nothing performs the mapping.

## Impact

Every component author who adds custom spec fields with underscores must use an awkward workaround:

```typescript
const rawSpec = spec as unknown as Record<string, unknown>;
const value = rawSpec["forage_postgresql"];
```

This is error-prone and defeats the purpose of generated type-safe interfaces.

## Root cause

`crates/forest-sdk-codegen/` (or wherever the TypeScript codegen lives) converts snake_case to camelCase for TypeScript convention, but the runtime JSON dispatch in `crates/forest/src/services/component_deno.rs` passes the CUE-evaluated JSON directly without any key transformation.

## Options

### Option A: Transform JSON keys at runtime (recommended)

In `component_deno.rs`, before passing the spec JSON to the component, recursively transform all keys from snake_case to camelCase. This makes the generated TypeScript interfaces accurate.

**Pros:** Generated code works as-is, TypeScript-idiomatic naming.
**Cons:** Runtime overhead (negligible), needs careful handling of nested objects.

### Option B: Generate snake_case TypeScript interfaces

Change the codegen to keep snake_case property names in TypeScript:

```typescript
export interface Spec {
  forage_postgresql?: Record<string, unknown>;
}
```

**Pros:** Zero runtime overhead, what-you-see-is-what-you-get.
**Cons:** Non-idiomatic TypeScript, but consistent with CUE.

### Option C: Generate both with runtime adapter

Generate camelCase interfaces but also generate a `fromRaw()` adapter that maps snake_case JSON to the camelCase interface. The `createRouter()` function would call `fromRaw()` before dispatching.

## Files to change

- TypeScript codegen module (likely in `crates/forest-sdk-codegen/`)
- OR `crates/forest/src/services/component_deno.rs` — add key transformation before passing spec to component

## Testing

- Create a component with snake_case spec fields
- Verify the generated TypeScript interface matches the runtime JSON
- Verify accessing properties via the generated interface works without casting
