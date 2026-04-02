# 008: Show `forest release create` hint after prepare/annotate

## Problem

The three-step release flow (prepare → annotate → release) is verbose for common cases. `forest release create` exists as a convenient shorthand but isn't mentioned in the output of the individual commands.

Developers discover it by reading `--help` or by accident.

## Fix

After `forest release prepare` succeeds, print:

```
Tip: use 'forest release create --env <env>' to prepare, annotate, and release in one step.
```

After `forest release annotate` succeeds, the current output already shows:

```
published artifact: some-slug
$ forest release some-slug --destination <...>
```

This is good but should additionally mention:

```
Or use 'forest release create --env <env>' next time to do all three steps at once.
```

## Files to change

- `crates/forest/src/cli/release/prepare.rs` — add hint after successful prepare
- `crates/forest/src/cli/release/annotate.rs` — add hint after successful annotate
