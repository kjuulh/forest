# 007: Document that `seal` command `cert` parameter is a file path

## Problem

The `seal` command's `cert` input is a file path to the PEM certificate, not the certificate content itself. This isn't documented anywhere — you have to read the sealed-secrets component's `lib.ts` source code to discover it.

When you pass the certificate content directly, kubeseal fails with a confusing error: `error: open true: no such file or directory`.

## Fix

### 1. Update the component spec description

In `components/kjuulh/sealed-secrets/forest.component.cue`:

```cue
#Commands: sdk.#ForestCommands & {
    seal: {
        description: "Add or update a sealed secret key"
        input: {
            env:   string
            key:   string
            value: string
            cert:  string  // Path to the PEM certificate file for kubeseal
        }
        output: {}
    }
}
```

### 2. Update the CLI help

When `forest run seal` is invoked, the help output should show:

```
seal — Add or update a sealed secret key

Arguments:
  --env    Target environment (e.g. "dev", "prod")
  --key    Secret key name (e.g. "DATABASE_URL")
  --value  Secret value (plaintext, will be encrypted)
  --cert   Path to the kubeseal PEM certificate file
```

### 3. Add validation in lib.ts

In `components/kjuulh/sealed-secrets/src/lib.ts`, before calling kubeseal, check that the cert path exists:

```typescript
try {
    await Deno.stat(opts.cert);
} catch {
    throw new Error(`cert file not found: ${opts.cert} (cert must be a file path, not certificate content)`);
}
```

## Files to change

- `components/kjuulh/sealed-secrets/forest.component.cue` — update description
- `components/kjuulh/sealed-secrets/src/lib.ts` — add file existence check with helpful error
- Documentation (if any docs site exists)
