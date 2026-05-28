# Forest - developer platform

Codify your development workflows; CI, deployments, component sharing as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

*Prerequisites*

- `gh` GitHub cli
- `cue` Cuelang (yaml and golang had a baby)

To install forest run the below command, it will install forest as a cli, and set your current profile to run against the production instance of forest.

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - \
  | FOREST_PROFILE='name=understory-prod,server=https://api.forest.understory.sh' bash
```

Next you need to add it to `.zshrc` to get full cli support

```bash
echo 'eval "$(forest shell zsh)"' >> ~/.zshrc
```

## Logging in

Either create an account or sign in, both can be done entirely in the terminal if wanted

```bash
forest auth login
```

### Keeping forest up to date

```bash
forest self update    # upgrade to latest
```

A one-line nag also prints at the end of every command when a newer release
exists (cached 24h; suppress with `FOREST_NO_UPDATE_CHECK=1` or `CI=true`).

## What's here

- [`apps/forest/`](apps/forest/) — the `forest` CLI and supporting libraries
- [`apps/forage/`](apps/forage/) — the managed web UI ([forest.understory.sh](https://forest.understory.sh)). Directory name remains `forage` for now; the crate hasn't been renamed.
