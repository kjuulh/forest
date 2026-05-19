# 006: Auto-detect commit SHA in `forest release annotate`

## Problem

Running `forest release annotate` without `--commit-sha` produces:

```
Error: commit sha not found : (TODO get from context)
```

The `TODO` in the error message indicates this is a known gap. The commit SHA should be auto-detected from the current git repository.

## Fix

In `crates/forest/src/cli/release/annotate.rs`, when `--commit-sha` is not provided:

1. Run `git rev-parse HEAD` in the current working directory
2. Use the result as the commit SHA
3. If git is not available or the directory is not a git repo, return a clear error:
   ```
   Error: --commit-sha is required (not in a git repository, or git not found)
   ```

Also auto-detect `--commit-branch` via `git branch --show-current` if not provided.

`forest release create` already does this correctly (it detects dirty trees and appends `-dirty`). The fix should reuse that logic for the standalone `annotate` command.

## Files to change

- `crates/forest/src/cli/release/annotate.rs` — add git auto-detection fallback
- Possibly extract git helpers from `crates/forest/src/cli/release/create.rs` into a shared module

## Testing

- Run `forest release annotate` in a git repo without `--commit-sha` — verify it auto-detects
- Run outside a git repo — verify clear error message
