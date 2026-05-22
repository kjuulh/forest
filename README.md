# Forest - developer platform

Codify your development workflows; CI, deployments, component sharing as
[CUE](https://cuelang.org/) manifests, then share them across your team.

## Install

To install forest run the below command, it will install forest as a cli, and set your current profile to run against the production instance of forest.

```bash
gh release download --repo understory-io/forest --pattern install.sh -O - \
  | FOREST_PROFILE='name=understory-prod,server=https://forest.understory.sh' bash
```


To pin to a specific version, pass it to both the download (so the script
itself comes from that release) and to the install script (so it grabs
the matching tarball). `bash -s --` forwards positional args when the
script comes in via stdin:

```bash
gh release download v0.1.7 --repo understory-io/forest --pattern install.sh -O - | bash -s -- v0.1.7
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
- [`apps/forage/`](apps/forage/) — the managed web UI ([forage.understory.sh](https://forage.understory.sh))
