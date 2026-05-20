# forest-contrib/init

Render a small project skeleton into the workflow's `work_dir`. The
bootstrap step you reach for first when standing up a fresh repo from
inside a Forest workflow.

## Inputs

- `template` — which template to render. v0.1 ships **`rust-cli`**
  only (an idiomatic Rust binary crate with `Cargo.toml`, `main.rs`,
  and a `.gitignore`). Unknown values fail loudly.
- Additional `with:` fields per template (e.g. project name).

## Behaviour

- Renders the chosen template's files into `context.work_dir`
  (defaults to `.`).
- Returns the list of files written so downstream steps can stage
  them.
- Idempotent within a workflow run: re-rendering overwrites the same
  paths.

## Roadmap

v0 ships the template inline. A future version will support
template-pack downloads once we have a story for distributing
templates as their own publishable artefact.

## Typical chain

```
forest-contrib/init      → forest-contrib/git-init  → forest-contrib/git-commit-push
   (scaffold files)         (turn into a repo)          (push to origin)
```
