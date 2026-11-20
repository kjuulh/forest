# Using Forest with Claude Code

The forest repository contains a Claude Code plugin with one skill, `forest`. Its instructions cover the real commands and flags, checks before publishing or releasing, and deployment evidence. Installing instructions does not guarantee that a model chooses the skill, its references, or the correct agent.

The skill only adds instructions. It runs nothing by itself, and it tells the agent to confirm with you before any command that changes shared state.

## Prerequisites

- Claude Code, with plugin support (`claude plugin --help` lists `marketplace` and `install`).
- Read access to `understory-io/forest` on GitHub (the repository is private).
- For the agent to act, the `forest` CLI installed and signed in ([Installation](installation.md), [Authentication](authentication.md)). The skill itself works without it.

## Install

The plugin files ship in v0.3.17 and on `main`. Installing from GitHub was verified from `main` (`b5175bd`) in an isolated config on 2026-10-21; for an unpublished checkout, use the local marketplace path below. Strict plugin validation and a local marketplace install are separate from model invocation, reference selection and agent choice, which remain **unverified**.

In Claude Code:

```text
/plugin marketplace add understory-io/forest
/plugin install forest@forest
```

Or from a shell:

```bash
claude plugin marketplace add understory-io/forest
claude plugin install forest@forest
```

The first `forest` is the plugin and the second is the marketplace, both declared in the repo's `.claude-plugin/marketplace.json`.

To try an unpublished change, point the marketplace at a local checkout instead:

```bash
claude plugin marketplace add /absolute/path/to/forest
claude plugin install forest@forest
```

## Check that it loaded

```bash
claude plugin details forest@forest
```

```text
Component inventory
  Skills (1)  forest
Projected token cost
  Always-on:   ~76 tok   added to every session
```

Only the skill's name and description are always-on metadata. The body is available when the skill is invoked and routes the agent to task-specific references rather than loading every topic. In a session, `/forest:forest` invokes it explicitly. `plugin details` confirms the installed inventory, not that a model followed the routing instructions.

## Update and remove

```bash
claude plugin marketplace update forest
claude plugin update forest@forest      # restart Claude Code to apply
claude plugin uninstall forest@forest
```

## If you also use the dx-claude plugins

`u-forest` (skill `u-forest:forest`) and `u-workflow`'s `using-forest` in `understory-io/dx-claude` are older Forest skills, and they compete with this one when both are installed. They don't redirect here. Prefer `forest:forest`, and invoke it by name if the older one fires instead.

## Changing the skill

The skill lives in [`apps/forest/plugin/skills/forest/`](https://github.com/understory-io/forest/tree/main/apps/forest/plugin/skills/forest): a short `SKILL.md` router plus `references/`. When CLI behaviour changes, update it in the same change. Then check it:

```bash
claude plugin validate --strict .
claude plugin validate --strict apps/forest/plugin
```
