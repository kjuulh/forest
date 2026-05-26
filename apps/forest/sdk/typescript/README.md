# @rawpotion/forest-sdk

TypeScript/Deno SDK for building [forest](https://src.rawpotion.io/rawpotion/forest) components.

Implements the Forest component protocol v2 — components are invoked as subprocesses with JSON lines over stdin/stdout.

## Install

```ts
import { runOnce, type ComponentService } from "jsr:@rawpotion/forest-sdk";
```

Or pin the version via `deno.json`:

```json
{
  "imports": {
    "@rawpotion/forest-sdk": "jsr:@rawpotion/forest-sdk@^0.1"
  }
}
```

## Usage

```ts
import { runOnce, type ComponentService } from "@rawpotion/forest-sdk";

interface Spec {
  name: string;
}

const service: ComponentService<Spec> = {
  methods() {
    return [{ name: "deploy", kind: "command" }];
  },
  async call(method, spec, _input, _context) {
    if (method === "deploy") {
      return { greeting: `hello, ${spec.name}` };
    }
    throw new Error(`unknown method: ${method}`);
  },
};

await runOnce(service);
```

See `forest-sdk.ts` for the full protocol surface (`callComponent`, `TemplateConfig`, error types).

## Versioning

This package tracks the `forest` CLI version. When cutting a release, bump
both `apps/forest/crates/forest/Cargo.toml` and `apps/forest/sdk/typescript/deno.json`
together — the JSR publish step refuses to push if the tag and `deno.json`
version disagree.
