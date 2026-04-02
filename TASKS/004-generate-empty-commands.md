# 004: `forest generate` should handle empty `#Commands`

## Problem

`forest generate --language typescript` fails with `lowering error: Commands schema has no properties` when a component defines:

```cue
#Commands: sdk.#ForestCommands & {}
```

This affects all resource-generator components (forage-postgresql, forage-nats, forage-s3) that only have deployment hooks and no commands.

Additionally, when such a component is listed as a dependency of another component, `forest generate` fails to produce the `deps/*.ts` client file for it, even though the component has valid hooks that can be called via `callComponent()`.

## Current workaround

- Write `forestgen.ts` manually for no-command components
- Write `deps/*.ts` client files manually for dependencies on no-command components

## Expected behavior

1. `forest generate` should produce a valid `forestgen.ts` with an empty `CommandHandler` interface:
   ```typescript
   export interface CommandHandler {}
   ```
2. `forest generate` should produce `deps/*.ts` clients for dependencies that have hooks but no commands, including the hook functions:
   ```typescript
   export function hooksForestDeploymentPrepare(spec: Spec, input: ...): Promise<...> {
     return callComponent("kjuulh/forage-nats", "hooks/forest/deployment/prepare", spec, input);
   }
   ```

## Files to change

- The CUE-to-TypeScript lowering logic (likely in `crates/forest-sdk-codegen/`)
- The dependency client generation logic (same crate)

## Testing

- Create a component with `#Commands: sdk.#ForestCommands & {}` and hooks
- Run `forest generate --language typescript`
- Verify `forestgen.ts` is generated with an empty `CommandHandler`
- Add this component as a dependency of another component
- Run `forest generate` on the parent component
- Verify `deps/<org>_<name>.ts` is generated with hook functions
