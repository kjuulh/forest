# forest

forest codifies development workflows — CI, deployments, component sharing, and
CLI tool distribution — as [CUE](https://cuelang.org/) manifests. `README.md`
covers installing it.

## Using forest

How to drive the CLI — `forest.cue` and components, `forest run`, shared tools,
publishing and prereleases, releases, pipelines, triggers and policies — lives in
one place: the `forest` skill in
[`apps/forest/plugin/skills/forest/`](apps/forest/plugin/skills/forest/SKILL.md).
Read its `SKILL.md` and the reference it routes to before running forest
commands.

Releases, publishes, approvals and trigger/pipeline/policy changes act
immediately on the active context: don't run them speculatively, and confirm
with the user first.

## Keep the skill current

The skill is how agents learn forest, so a change that alters what a user or
agent sees must update it **in the same change**. That covers:

- a CLI command, flag, default, output line, error message or exit code;
- server release, trigger, pipeline or policy behaviour;
- manifest schema (`forest.cue`, `forest.component.cue`, the SDK spec);
- version, prerelease, lockfile or digest semantics, and tool selection.

How:

1. Edit the reference the change belongs to (`references/*.md`). Keep
   `SKILL.md` a short router: add to it only facts every task needs.
2. Label behaviour that isn't in a released version yet as "after v<last
   release>", and say how to detect it (usually a flag in `--help`). Keep the
   older behaviour while users may still run it. When a release ships it,
   relabel to that version.
3. Remove or correct anything the change makes false. A fixed bug must not
   stay taught as current behaviour.
4. Update the matching page under `apps/forest/docs/docs/` too.
5. Run `claude plugin validate --strict .` and
   `claude plugin validate --strict apps/forest/plugin`. If an example
   changed, prove it with the real CLI instead of copying it from the diff.

## This repo

- [`apps/forest/`](apps/forest/) — the `forest` CLI and its libraries (`apps/forest/CLAUDE.md` has the development rules)
- [`apps/forage/`](apps/forage/) — the managed web UI at [forest.understory.sh](https://forest.understory.sh)
- [`apps/forest/examples/`](apps/forest/examples/) — a worked example per project shape
- [`apps/forest/components/forest/sdk/spec.cue`](apps/forest/components/forest/sdk/spec.cue) — the published SDK schema; the authoritative definition of every manifest type
- [`apps/forest/docs/docs/`](apps/forest/docs/docs/) — the docs site; check a command's `--help` before copying an example

`apps/forest/cue/forest-sdk/spec.cue` is an older copy of that schema and lags
behind it — read the `components/` one.

`cue` and `gh` must be on `PATH`. Deploying forest and forage is CI's job on push
to `main`; see `README.md`.
