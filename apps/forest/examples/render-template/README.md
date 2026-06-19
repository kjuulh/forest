# forest-contrib/render-template

Walk a source directory and copy it into a destination, interpolating
`{{var}}` placeholders along the way. The Forest equivalent of running
`cookiecutter` or `copier` from a workflow.

## Inputs

- `src` — directory to render from.
- `dest` — directory to render into.
- `vars` — JSON object of `name → value`. Values must be scalars
  (string / number / bool / null); objects and arrays are rejected so
  the substitution stays unambiguous.

## What gets interpolated

- **File contents** for UTF-8 files. Binary files pass through
  byte-identical.
- **Path components** themselves — so a source path like
  `src/{{project}}/main.rs` lands at `dest/<actual project>/main.rs`.

## Strictness

Unknown placeholders abort with an error rather than rendering as
empty strings. Typos in `vars` surface fast.

## Other guarantees

- Executable bits are preserved on Unix.
- `dest` is created if it doesn't exist; existing files are
  overwritten.

## Output

`files_rendered` (count), `src`, `dest`.
