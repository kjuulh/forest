# CLI Reference

Complete reference for the `forest` command-line tool.

## Global Options

| Option | Description |
|--------|-------------|
| `--version` | Print version |
| `--help` | Print help |

---

## `forest init`

Scaffold a new project or component from a starter template.

```bash
forest init [STARTER] [--dest <PATH>]
```

| Argument | Description |
|----------|-------------|
| `STARTER` | Starter template name (optional, prompts if omitted) |
| `--dest`, `--destination` | Target directory (default: `.`) |

---

## `forest add`

Add a component dependency to the project.

```bash
forest add <COMPONENT> [--path <PATH>]
```

| Argument | Description |
|----------|-------------|
| `COMPONENT` | Component reference: `org/name` or `org/name@version` |
| `--path` | Use a local path instead of registry version |

**Examples:**

```bash
forest add forest-contrib/kubernetes-service
forest add forest-contrib/kubernetes-service@0.2.0
forest add forest-contrib/kubernetes-service --path ../local-dev
```

---

## `forest build`

Build the component binary for all configured platforms.

```bash
forest build
```

Reads `forest.cue` and `spec.cue` to determine component name, version, and target architectures. Outputs binaries to `~/.cache/forest/components/bin/`.

---

## `forest generate`

Generate SDK code from the CUE component spec.

```bash
forest generate --output <DIR> [--language <LANG>]
```

| Option | Description |
|--------|-------------|
| `--output` | Output directory for generated code (required) |
| `--language` | Target language: `rust`, `typescript`, `deno`, `ts` (auto-detected if omitted) |

---

## `forest publish`

Publish the component to the Forest registry.

```bash
forest publish
```

Uploads the compiled binary, CUE spec files, and component manifest. Requires `forest build` to be run first.

---

## `forest validate`

Validate project configuration against component specs.

```bash
forest validate
```

Checks that project config matches component schemas and verifies contract coverage (which deployment hooks are fulfilled).

---

## `forest update`

Update dependencies to the latest versions matching the spec.

```bash
forest update [COMPONENT]
```

| Argument | Description |
|----------|-------------|
| `COMPONENT` | Specific component to update (`org/name`). If omitted, updates all. |

---

## `forest run`

Run a project or component command.

```bash
forest run <COMMAND> [ARGS...]
```

Commands are dynamically discovered from component definitions. Supports both short and qualified names:

```bash
forest run status               # Short name
forest run my-component:status  # Fully qualified
```

All trailing arguments are passed through to the component.

---

## `forest release`

Manage the release lifecycle.

### `forest release prepare`

Generate deployment manifests by invoking component hooks.

```bash
forest release prepare
```

### `forest release annotate`

Upload artifacts and create a release annotation.

```bash
forest release annotate [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--organisation`, `-o` | Organisation name (required) |
| `--project-name` | Project name (required) |
| `--context-title` | Release title (required) |
| `--context-description` | Release description |
| `--context-web` | Web link to the change |
| `--context-pr` | Pull request link |
| `--commit-sha` | Commit SHA |
| `--commit-branch` | Source branch |
| `--commit-message` | Commit message |
| `--source-type` | Source type (e.g., `ci`, `manual`) |
| `--source-username` | Who triggered the release |
| `--source-email` | Triggerer's email |
| `--run-url` | Link to CI run |
| `--metadata` | Key-value metadata (repeatable) |

### `forest release release`

Execute the release to destinations.

```bash
forest release release [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--organisation`, `-o` | Organisation name |
| `--project`, `-p` | Project name |
| `--environment`, `-e`, `--env` | Target environment |
| `--destination`, `-d` | Specific destination(s) (repeatable) |
| `--ref`, `-r` | Artifact reference |
| `--artifact-id`, `--id` | Artifact ID |
| `--force` | Cancel queued releases, jump to front |
| `--pipeline` | Use the project's release pipeline |
| `--no-wait` | Don't stream progress |

### `forest release create`

Combined command: prepare, annotate (without triggers), and release.

```bash
forest release create --environment <ENV> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--environment`, `-e`, `--env` | Target environment (required) |
| `--title` | Release title (default: latest git commit subject) |
| `--description` | Release description (default: git commit body) |
| `--organisation`, `-o` | Organisation (auto-detected from `forest.cue`) |
| `--project`, `-p` | Project (auto-detected from `forest.cue`) |
| `--commit-sha` | Commit SHA (auto-detected from HEAD) |

---

## `forest project`

Manage projects.

### `forest project create`

```bash
forest project create --organisation <ORG> --name <NAME>
```

### `forest project init`

Initialize from `forest.cue`.

```bash
forest project init
```

### `forest project publish`

Publish project configuration.

```bash
forest project publish
```

### `forest project list`

```bash
forest project list --organisation <ORG>
```

### `forest project releases`

Show current release state per destination.

```bash
forest project releases --organisation <ORG> --project <PROJECT>
```

### `forest project trigger`

Manage release triggers. Subcommands: `create`, `list`, `update`, `delete`.

### `forest project policy`

Manage deployment policies. Subcommands: `create`, `list`, `update`, `delete`, `evaluate`.

### `forest project pipeline`

Manage release pipelines. Subcommands: `create`, `list`, `update`, `delete`.

---

## `forest destination`

Manage deployment destinations.

### `forest destination create`

```bash
forest destination create --organisation <ORG> --name <NAME> --environment <ENV> --type <TYPE>
```

### `forest destination update`

```bash
forest destination update --organisation <ORG> --name <NAME>
```

### `forest destination delete`

```bash
forest destination delete --organisation <ORG> --name <NAME>
```

### `forest destination list`

```bash
forest destination list --organisation <ORG>
```

### `forest destination types`

List available destination types.

```bash
forest destination types
```

---

## `forest environment`

Manage environments.

### `forest environment create`

```bash
forest environment create --organisation <ORG> --name <NAME>
```

### `forest environment list`

```bash
forest environment list --organisation <ORG>
```

### `forest environment get`

```bash
forest environment get --organisation <ORG> --name <NAME>
```

### `forest environment update`

```bash
forest environment update --organisation <ORG> --name <NAME>
```

### `forest environment delete`

```bash
forest environment delete --organisation <ORG> --name <NAME>
```

---

## `forest organisation`

Manage organisations and members.

```bash
forest organisation list
forest organisation members --organisation <ORG>
```

---

## `forest components`

Browse and manage components in the registry.

### `forest components init`

Scaffold a new component from a template.

```bash
forest components init <NAME> [--organisation <ORG>] [--language <LANG>] [--output <DIR>]
```

| Option | Default | Description |
|--------|---------|-------------|
| `NAME` | | Component name (required) |
| `--organisation` | `forest-contrib` | Organisation namespace |
| `--language` | `rust` | Implementation language |
| `--output` | `.` | Output directory |

### `forest components list`

Search and list components in the registry.

```bash
forest components list
```

---

## `forest auth`

Authentication commands.

```bash
forest auth register    # Create a new account
forest auth login       # Authenticate with the server
forest auth logout      # Log out
forest auth status      # Show current auth status
forest auth token       # Manage personal access tokens
```

---

## `forest notifications`

Listen for and manage notifications.

```bash
forest notifications subscribe [OPTIONS]
```
