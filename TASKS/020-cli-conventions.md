# 020: Forest CLI Conventions — audit + alignment plan

Status: Spec — pre-implementation. Backward-compatible migration recommended (add new aliases, keep old surfaces hidden).

## 0. Why

The CLI has accreted across many feature spec passes. Different commands use different verbs for the same operation, different flag spellings for the same concept, and different output styles. This makes the surface harder to learn and to script against. This doc takes inventory, identifies the divergences, and proposes a single coherent convention with a backward-compatible migration path.

## 1. Inventory (today)

### 1.1 Top level

| Command | Purpose |
|---|---|
| `forest init` | Scaffold a new project from a template (filesystem) |
| `forest add` | Add a component dep to current project's forest.cue |
| `forest build` | Build component binary |
| `forest generate` | Codegen from `forest.component.cue` |
| `forest publish` | Publish component to registry |
| `forest validate` | Validate forest.cue against component specs |
| `forest update` | Update deps in forest.cue / forest.lock |
| `forest run` | Run a component command (e.g. `forest run status`) |
| `forest release` | Prepare / annotate / release |
| `forest project …` | Manage projects |
| `forest destination …` | Manage destinations |
| `forest environment …` | Manage environments |
| `forest organisation …` | Manage orgs + members |
| `forest notifications …` | Notifications |
| `forest components …` | Browse the registry |
| `forest docs` | Show docs |
| `forest auth …` | Login / register / status / tokens |
| `forest context …` | kubectl-style profile switching |
| `forest eval …` | Shell integration script |
| `forest tool …` | Helpers for authoring external tool manifests |
| `forest global …` | Global tools (mise-style) |

### 1.2 Per-noun subcommand surfaces

| Noun | create | read-one | read-many | update | delete | other |
|---|---|---|---|---|---|---|
| project | `create` | — | `list` | — | — | `init` (scaffold), `publish`, `releases`, `trigger`, `policy`, `pipeline` |
| destination | `create` | — | `list` | `update` | `delete` | `types` |
| environment | `create` | `get` | `list` | `update` | `delete` | — |
| organisation | `create` | `get` | `search` | — | — | `member` |
| org member | `add` | — | `list` | `update-role` | `remove` | — |
| trigger | `create` | — | `list` | `update` | `delete` | — |
| policy | `create` | — | `list` | `update` | `delete` | `evaluate` |
| pipeline | `create` | — | `list` | `update` | `delete` | — |
| context | `create` | `active` | `list` | `set-server` | `delete` | `use`, `rename` |
| auth token | `create` | — | `list` | — | `delete` | — |
| component | (`publish`) | `show` | `list` | — | — | — |
| global tool | `add` | `which` | `list` | `update`, `sync` | `remove` | `run`, `verify`, `ban`, `unban`, `pin`, `unpin` |
| notification | — | — | `list` | — | — | `listen`, `preferences` |

## 2. Findings

### 2.1 Create-verb divergence

| Verb | Used by | Semantic |
|---|---|---|
| `create` | project, destination, environment, organisation, trigger, policy, pipeline, context, auth token | Register a new resource on the server. |
| `add` | `forest add` (component dep), `forest global add` (tool dep), `organisation member add` | Add something *to* something else (membership, dependency). |
| `init` | `forest init` (project scaffold), `forest project init`, `forest components init`, `forest global init` | Scaffold files on disk from a template. |
| `register` | `forest auth register` | Account-level signup. Justified as a domain term. |
| `publish` | `forest publish`, `forest project publish` | Push artefact to registry. |

The `create`/`add`/`init` split is meaningful — `create` is a registry write, `add` is a relationship write, `init` is a filesystem write — but it isn't documented anywhere and the names overlap. `forest init` and `forest global init` both scaffold a project, but `forest project init` does something else entirely (currently undocumented — empty help).

### 2.2 Destroy-verb divergence

| Verb | Used by |
|---|---|
| `delete` | destination, environment, trigger, policy, pipeline, context, auth token, project (TBD) |
| `remove` | `forest global remove`, `organisation member remove` |
| `ban` / `unban` | global tool (special semantic, fine) |
| `verify` | global cache (not a destroy, but related) |

The `delete` vs `remove` split mirrors `create` vs `add`: `remove` is for removing a relationship; `delete` is for destroying a resource. Consistent enough.

### 2.3 Read-one verb divergence

| Verb | Used by |
|---|---|
| `get` | organisation, environment |
| `show` | components |
| `active` | context (special: prints the active one, not by name) |
| `which` | global tool (path lookup) |
| `status` | auth (not a noun read; session state) |

Three different verbs for "show one thing": `get`, `show`, plus the noun-specific `active`/`which`/`status`. **No single project-wide convention** for inspecting a single resource by name.

### 2.4 Read-many verb (consistent ✓)

`list` everywhere. Good.

### 2.5 Update verbs

| Verb | Used by |
|---|---|
| `update` | destination, environment, trigger, policy, pipeline (whole-resource) |
| `update-role` | org member (kebab compound) |
| `set-server` | context (kebab compound) |
| `set` | `forest global set` (user kv config) |
| `use` | context (switch the active marker; orthogonal to "edit fields") |

`update-X` vs `set-X` for editing a single field is inconsistent.

### 2.6 Flag spelling

| Concept | Flag spellings observed |
|---|---|
| organisation | `--org` (org member add, components list), `--organisation` (none currently) |
| user | `--user`, `--username` (auth register), `--user-id` (auth status) |
| name (of the resource being created) | `--name` (mostly), positional in some |
| version | positional in `pin`, `add @<ver>` style in `global add`, `--version` elsewhere |
| context override | `--context` (global ✓) |
| output format | `--format pretty` (only on some org commands) |

`--org` is the de-facto short form. Should be added as an alias everywhere `--organisation` (or implicit) is used.

### 2.7 Reference shape

| Style | Used by | Pros / cons |
|---|---|---|
| `<org>/<name>[@<version>]` | global add, global remove, global run, global which, components show | Single positional, scriptable. |
| `--name X` (implicit org from project) | destination create, environment create | Project-context-aware. |
| `--org X --name Y` | organisation member add | Explicit, harder to type. |

Choice depends on whether the command makes sense outside a project. For *registry* operations, `<org>/<name>` should win. For *project* operations, implicit-org-from-cwd makes sense.

### 2.8 Output format

`--format pretty` exists on some organisation commands. Most commands don't accept `--format`. There is no `--format json` for scripting.

### 2.9 Help text quality

Hand-spotted gaps:
- `forest project init` and `forest project publish` and `forest project list` have **no help text at all** (empty `Commands:` rows in the audit).
- `forest destination create / update / delete / list` similarly empty.
- `forest tool hash` has good help.
- `forest context …` has good help.
- `forest global …` has good help.

Recent commands have rich help; older commands have placeholder/empty descriptions.

### 2.10 Output channels (stdout vs stderr)

Inconsistent: some commands print success notices to stdout (which can break pipelines), some to stderr.

### 2.11 Discoverability — "init" ambiguity

`forest init`, `forest project init`, `forest components init`, `forest global init` all scaffold things on disk but operate on different scopes. No one-liner tells the user when to use which.

## 3. Proposed conventions

### 3.1 Verb catalogue

| Operation | Canonical verb | When to use |
|---|---|---|
| Register a resource on the server | `create` | The server allocates an ID, returns it. |
| Bind/attach an existing thing to a container | `add` | Membership, dependency, subscription. |
| Scaffold files locally from a template | `init` | Filesystem only, no network. |
| Inspect one resource | `show <ref>` | Detail page. **One verb, everywhere.** |
| List many resources | `list` | Already consistent. |
| Edit the whole resource | `update` | Idempotent overwrite. |
| Edit one field | `set-<field>` | `set-server`, `set-role`, `set-description`. |
| Destroy a resource | `delete` | Server-side, irreversible. |
| Unbind from a container | `remove` | Counterpart of `add`. |
| Trigger/run a verb that isn't CRUD | the verb itself | `forest release prepare`, `forest global run`. |

`get` ≡ `show`. We collapse on `show` because it pairs naturally with `list` (`show one thing`, `list many things`), and because `get` is overloaded (`forest organisation get` returns a single org by id-or-name, exactly the same as `show`).

### 3.2 Flag standards

- Organisation: `--org` (short) ≡ `--organisation` (long alias). Both work; `--org` is the documented form.
- User reference: `--user <id-or-username>` (single flag accepting either UUID or username). Drop `--user-id`/`--username` distinction at the CLI level (server-side resolution handles both).
- Resource name: `--name` for create, positional for show/delete/update where the name uniquely identifies it.
- Version: `--version` for create/update flags; `@<ver>` suffix for refs (`org/name@1.2.3`).
- Output format: `--format pretty|json|wide|name` available on every list/show command. `pretty` (default), `json` for scripting, `wide` shows extra columns, `name` prints only the resource name (for piping into other commands).

### 3.3 Reference shape

- **Registry-level**: `<org>/<name>[@<version>]` as a single positional. Used by `forest components show`, `forest global add`, `forest global run`, etc.
- **Project-level**: implicit org+project from `forest.cue` in cwd; `--name` for the resource within the project; `--project <name>` to override.
- **Cross-cutting (members, orgs)**: `--org` + `--user` flags.

### 3.4 Help-text quality bar

Every command + subcommand + flag MUST have:
- A one-line description (used in the parent's `Commands:` table).
- A long description (used in `--help`) with at least one example invocation for commands that take args.
- For non-obvious flags: an example value.

### 3.5 Stdout vs stderr

- **stdout** is the command's *result* (the data the user piped this command for). For mutations with no useful result, stdout is empty.
- **stderr** is progress, status, "X created", warnings, errors.
- `--format json` always writes to stdout, never interspersed with progress.

### 3.6 "init" disambiguation

- `forest init` is the top-level user-facing scaffolder; subsumes the others where it can.
- `forest global init` should be renamed `forest global scaffold` OR removed (`forest init` covers it).
- `forest project init` should be removed (it's apparently dead code with empty help).
- `forest components init` stays — it scaffolds a *component*, distinct from a *project*.

## 4. Migration plan (backward-compatible)

### Tier 1 — additive aliases (no breakage)

Add now via `#[command(alias = "old-name")]` or `#[arg(alias = "old-flag")]`:

| Current | Canonical | Alias rule |
|---|---|---|
| `forest organisation get` | `forest organisation show` | `get` becomes a hidden alias of `show` |
| `forest environment get` | `forest environment show` | same |
| `forest organisation member update-role` | `forest organisation member set-role` | `update-role` → hidden alias |
| `--organisation` (where used) | `--org` | Both work, `--org` documented |
| `--username` | `--user` | Both accepted, `--user` documented |

### Tier 2 — fill in missing help text

Every command currently shipping empty help text gets a one-liner + long description. Mechanical, no behaviour change.

### Tier 3 — add `--format pretty|json|wide|name` to list/show

A small `OutputFormat` enum + a shared formatter trait. Implement first for `forest global list`, `forest components list`, `forest organisation list`, then propagate.

### Tier 4 — collapse the redundant `init` commands

- Rename `forest global init` → `forest global scaffold` (with `init` as hidden alias).
- Remove `forest project init` if it's dead; otherwise document what it does.

### Tier 5 — consistent stdout/stderr split

Audit each command; move success notices ("created X") to stderr; reserve stdout for data.

## 5. Out of scope

- A CLI "policy" lint (`cargo dist`-style) that enforces conventions in CI. Worth doing later.
- Shell completions for the new aliases.
- Manpage generation.

## 6. Risk + rollout

- All Tier 1 + Tier 2 changes are additive and reversible. Land them as one PR each (verb aliases, flag aliases, help text fills) — easy review.
- Tier 3 changes a bunch of output, but `--format pretty` (default) stays byte-equal to today's output. The new `--format json` is new surface, not a change.
- Tier 4 removes/renames commands. Keep the old names as hidden aliases for at least one minor release; only delete on a major bump.
- Tier 5 changes which channel some text lands on. Could break a script that grepped stdout for "created". Acceptable in a minor release if announced.

## 7. Sequence

1. Tier 2 first (help text fills) — pure docs win, no behaviour change.
2. Tier 1 next (verb + flag aliases) — small PR per noun.
3. Tier 3 (`--format` framework) — design the trait, roll out per command.
4. Tier 4 (`init` cleanup) — small.
5. Tier 5 (stdout/stderr) — audit + per-command fixes; this is where regressions are most likely.
