# 019: Forest Context — kubectl-style profile switcher

Status: Spec — pre-implementation.

## 0. Why

Today there is exactly one Forest auth state at `$XDG_DATA_HOME/forest/user-state.json`. Every `forest …` invocation reads it. That means:

- You cannot be logged into local-dev as one user and production as another at the same time.
- `FOREST_SERVER` can be overridden per-invocation, but the auth state cannot — so pointing at a different server with the wrong token gives a 401.
- Tests, CI runs, and example walkthroughs all pollute the same global file (we saw this in cycle 9 verification).
- Switching between users requires `forest auth logout && forest auth login` — destructive, and you lose the other session.

A **Context** is a named bundle of *(server URL, auth state)* (plus optional defaults like organisation). You can have many; one is active at a time; commands operate against the active one unless overridden.

Direct analogue: `kubectl config` contexts, `gh auth switch`, `awscli` profiles.

## 1. Behavioural contract

### 1.1 What a context is

```cue
#Context: {
    // Unique name. Same validation rules as tool names (§018 §1a.1).
    name: string

    // Server gRPC URL. Required.
    server: string

    // Optional default org for commands that take --org. Future polish.
    default_organisation?: string

    // Created-at timestamp for "context list" ordering.
    created_at: string
}
```

Auth state (`user-state.json` content — access_token, refresh_access, user_id, username, emails, refresh_after) lives **separately, in a per-context directory**. The context registry only stores metadata.

### 1.2 Storage layout

```
$XDG_DATA_HOME/forest/
├── contexts.json                      # registry: known contexts + active name
└── contexts/
    ├── default/
    │   └── user-state.json            # auth state for "default"
    ├── prod/
    │   └── user-state.json
    └── localdev/
        └── user-state.json
```

`contexts.json` shape:

```json
{
  "active": "default",
  "contexts": [
    {"name": "default",  "server": "http://localhost:4040", "created_at": "..."},
    {"name": "prod",     "server": "https://forest.example.com", "created_at": "..."},
    {"name": "localdev", "server": "http://localhost:4040", "created_at": "..."}
  ]
}
```

### 1.3 Resolution order

For every command that needs `(server_url, auth_token)`:

1. Per-invocation `--context <name>` flag — overrides everything else for this command.
2. `FOREST_CONTEXT` env var — same effect as `--context`, scoped to the shell.
3. The `active` field in `contexts.json`.
4. Bootstrap: if `contexts.json` doesn't exist but the legacy single `user-state.json` does, migrate it to a `default` context (one-time, see §1.5) and use that.
5. Bootstrap: if neither exists, auto-create a `default` context with `server = http://localhost:4040` and let the user fix it via `forest context create` or env vars.

`FOREST_SERVER` is still honoured — when set, it overrides the *server URL* of the resolved context but the auth token comes from the resolved context. This matches kubectl (`--server` vs context).

### 1.4 CLI surface

| Command | What it does |
|---|---|
| `forest context list` | Table: name, server URL, user (if logged in), [active] marker. |
| `forest context active` | Print active context name + URL on one line. |
| `forest context use <name>` | Switch the `active` field in contexts.json. |
| `forest context create <name> --server <url>` | Add a new context entry. Auth state empty until `forest auth login` against it. `--use` flag to immediately switch active. |
| `forest context delete <name>` | Remove the context dir + registry entry. Refuses to delete the active context unless `--force`. Doesn't delete the directory on `--keep-data`. |
| `forest context rename <old> <new>` | Rename in registry + move the dir. |
| `forest context set-server <name> <url>` | Update the server URL of a context. |

All `forest auth …` and `forest global …` commands accept a global `--context <name>` flag (overrides active for that invocation).

### 1.5 Migration of legacy single user-state.json

On first read where `$XDG_DATA_HOME/forest/contexts.json` is absent:

1. If `$XDG_DATA_HOME/forest/user-state.json` exists with valid JSON → move it to `$XDG_DATA_HOME/forest/contexts/default/user-state.json` and write a `contexts.json` with one `default` entry (server = `$FOREST_SERVER` env if set, else `http://localhost:4040`), `active = default`.
2. If no legacy file → write a `contexts.json` with one empty `default` context (no auth).

The move is **atomic** (rename, not copy); the legacy path is gone afterward. A one-line stderr notice is printed.

### 1.6 Edge cases

| # | Scenario | Behaviour |
|---|---|---|
| C1 | `forest auth login` against an unknown context (via `--context`) | Auto-creates the context if `--server` is also provided; else errors with hint. |
| C2 | `forest context delete <active>` | Refuses unless `--force`; suggests `forest context use <other>` first. |
| C3 | `forest context use <unknown>` | Lists known contexts, errors. |
| C4 | `forest context create` with a name that already exists | Errors, prints the existing entry. |
| C5 | `forest context list` with no auth in any context | Shows `(not logged in)` in the user column. |
| C6 | Two processes write `contexts.json` concurrently | `tempfile + fsync + rename` (same primitive as `forest.cue`). |
| C7 | Name validation | Same regex as tool names: `^[a-zA-Z][a-zA-Z0-9._-]{0,63}$`. |
| C8 | `forest context rename default <x>` then no contexts | Allowed; the next op auto-creates a fresh `default` on bootstrap. |

### 1.7 Out of scope (later)

- Per-context default organisation.
- Per-context default project.
- Per-context environment variables.
- Importing/exporting contexts (`forest context export`).
- Federated SSO / multi-server token sharing.

## 2. Verification

- Unit tests on the storage layer: round-trip contexts.json, atomic write, name validation, active-context guard on delete.
- Integration: live registration under two named contexts against the same server, verifying tokens stay isolated.
- Live walkthrough: `forest context create local --server http://localhost:4040 --use`, `forest auth register`, `forest auth status` shows the new user under `local`, switch back to `default`, verify the old user is still authenticated.

## 3. Files touched

- `crates/forest/src/contexts.rs` (new) — storage layer.
- `crates/forest/src/cli/context.rs` (new) — CLI subcommands.
- `crates/forest/src/cli.rs` — register `Context(ContextCommand)`, add global `--context` flag.
- `crates/forest/src/user_state.rs` — resolve path via active context.
- `crates/forest/src/state.rs` — `Config` grows `context: Option<String>` with `env = "FOREST_CONTEXT"`.
- `crates/forest/src/grpc.rs` — resolve server URL via active context.
