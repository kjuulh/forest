# 003: Fix `forest run seal` corrupting multi-line values

## Problem

When sealing multi-line content (e.g. NATS credentials files, PEM certificates, SSH keys), the `forest run seal` command corrupts the value. A multi-line NATS creds file gets stored as `true` in the unsealed secret.

## Reproduction

```bash
# This produces a corrupted sealed value:
CREDS=$(cat /path/to/sys.creds)
forest run seal --env dev --key NATS_SYS_CREDS --value "$CREDS" --cert /path/to/cert.pem

# The unsealed secret contains "true" instead of the creds file content
kubectl get secret my-secrets -o jsonpath='{.data.NATS_SYS_CREDS}' | base64 -d
# Output: true
```

## Root cause

The seal command passes the `--value` argument through the forest component RPC layer (forest → deno component → kubeseal). Somewhere in this chain, the multi-line string is being truncated or evaluated as a boolean.

Likely locations:
1. **CUE evaluation** — CUE may interpret the value before passing it to the component
2. **JSON serialization** — The component call JSON payload may not properly escape newlines
3. **Deno argument parsing** — The deno component may receive a truncated value
4. **kubeseal stdin pipe** — The sealed-secrets `lib.ts` pipes the value to kubeseal via stdin; the value might be corrupted before reaching stdin

## Investigation steps

1. Add debug logging to `components/kjuulh/sealed-secrets/src/lib.ts` in the `sealSecret()` function to print the received value length
2. Check if the value arrives intact at the deno component
3. Check the JSON payload in `crates/forest/src/services/component_deno.rs` when calling the component

## Fix options

### Option A: Base64-encode values in the RPC layer

Before passing the value through CUE/JSON, base64-encode it. The sealed-secrets component decodes it before passing to kubeseal.

### Option B: Add `--value-file` flag

Add a `--value-file <path>` option to `forest run seal` that reads the value from a file, bypassing the CLI argument parsing entirely:

```bash
forest run seal --env dev --key NATS_SYS_CREDS --value-file /path/to/sys.creds --cert /path/to/cert.pem
```

The file content would be read directly by the sealed-secrets component rather than passed through the RPC layer.

### Option C: Fix the escaping in the component call chain

Identify where the newlines are being lost and fix the serialization. This is the cleanest fix but requires deep investigation into the CUE → JSON → deno call chain.

## Files to change

- `crates/forest/src/cli/run.rs` — command input parsing
- `crates/forest/src/services/component_deno.rs` — component call serialization
- `components/kjuulh/sealed-secrets/src/lib.ts` — value handling
- `components/kjuulh/sealed-secrets/forest.component.cue` — potentially add `value_file` input field

## Testing

- Seal a multi-line value (NATS creds file with `-----BEGIN/END-----` markers)
- Verify the unsealed secret contains the exact original content
- Seal a value containing special characters (`=`, `+`, `/`, newlines)
- Verify roundtrip integrity
